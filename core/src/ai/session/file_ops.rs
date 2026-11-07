//! file_ops 子模块：match_file / patch_file 完整实现。
//!
//! 拆出来的理由：file_ops 是一坨自成体系的复杂逻辑 ——
//! - 远端能力探测（python3 / perl / diff）
//! - Base64 数据封装让脚本和多行参数保持单行，兼容标准 sh 与 Windows shell
//! - 5 张审批卡片编排（match × 1，patch × 4：cp → modify → diff → mv）
//!
//! 与会话主框架（dialogue_turn / run_command / SFTP）耦合面很窄（只通过 Actor 的几个
//! pub(super) 方法和字段），按职责分离更利于阅读。
//!
//! **核心不变量**：所有送进 PTY 的 file_ops 命令在 shell 视角下必须是**单行**（无字面
//! `\n` / `\r`），避开终端多行粘贴与转义字符注入；解码在目标解释器中完成。

use base64::Engine;
use serde_json::json;

use crate::ai::audit::AuditKind;
use crate::ai::llm::{ChatMessage, ToolCall};
use crate::ai::sanitize;
use crate::ai::shell::ShellKind;
use crate::ai::tools::{MatchFileInput, PatchFileInput, MATCH_CONTEXT_DEFAULT, MATCH_CONTEXT_MAX};
use crate::error::{AppError, AppResult};

use super::{Actor, CommandOutcome};

// ─── 远端能力探测 ─────────────────────────────────────────────────

/// 远端 file_ops 工具能力。只缓存完整可用的工具链，失败或缺项下次重试。
///
/// 设计原则：rssh 后端不再 cat 整文件回 PTY（避免 ANSI/scrollback/buffer 丢内容），
/// 改为让远端预制脚本读文件 + 算 count/context + 写 tmp，只回小 JSON。
/// 所有 shell 共用 Python 3 / Perl 脚本；patch 另外需要外部 diff。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RemoteCapabilities {
    /// `python3`（首选）
    pub(super) python3: bool,
    /// `perl`（降级）—— `\Q...\E` 字面匹配
    pub(super) perl: bool,
    /// `diff -u`（patch_file 出审批 diff 必备）
    pub(super) diff: bool,
}

/// 探测命令：一行拿三个工具的可用性。输出形如 `py3=1 perl=1 diff=1`。
const PROBE_CMD: &str = r#"echo "py3=$(python3 -c 'import sys;sys.exit(sys.version_info[0] != 3)' >/dev/null 2>&1 && echo 1 || echo 0) perl=$(command -v perl >/dev/null 2>&1 && echo 1 || echo 0) diff=$(command -v diff >/dev/null 2>&1 && echo 1 || echo 0)""#;

/// 解析探测命令输出 —— 找含 `py3=` `perl=` `diff=` 的那一行。
fn parse_capabilities(output: &str) -> Option<RemoteCapabilities> {
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if let [python3, perl, diff] = fields.as_slice() {
            let flag = |value: &str, prefix: &str| match value.strip_prefix(prefix) {
                Some("0") => Some(false),
                Some("1") => Some(true),
                _ => None,
            };
            if let (Some(python3), Some(perl), Some(diff)) = (
                flag(python3, "py3="),
                flag(perl, "perl="),
                flag(diff, "diff="),
            ) {
                return Some(RemoteCapabilities {
                    python3,
                    perl,
                    diff,
                });
            }
        }
    }
    None
}

// ─── JSON marker ─────────────────────────────────────────────────

/// JSON 输出包裹 marker：脚本把结果包在两个 marker 之间输出，
/// rssh 后端用此 marker 从 PTY 字节流里精准切出 JSON，规避 shell prompt /
/// ANSI 序列 / 命令回显的干扰。
const JSON_MARKER: &str = "__RSSH_JSON__";

/// 从 PTY 原始输出里抽出由 `JSON_MARKER` 包裹的内容。
/// 找不到一对独立成行的 marker 返回 None。
///
/// **按"独立成行"匹配**，不是 rfind 子串。脚本字面量里有 `M="__RSSH_JSON__"` 这行，
/// 脚本协议把 marker 输出为**整行**（前后都是 `\n`）。如果用 rfind：
///   1. 脚本源码 `M = "..."` 那行 echo 回显（marker 子串嵌在 `M = "<marker>"`）
///   2. 脚本输出的开头 marker（整行）
///   3. 脚本输出的结尾 marker（整行）
///   4. **更危险**：match_file 的 context 字段如果含 marker 字面（用户在文件里写了
///      `__RSSH_JSON__` 注释），子串扫到的位置会在 JSON 内部切，破坏解析。
///
/// 按行匹配杜绝以上风险：只有"整行 trim 后等于 marker"的位置才算锚点。脚本输出的
/// marker 一定独占一行；echo 回显里的字面量永远嵌在更长的源码行里，不会误匹配；
/// JSON payload 里的 marker（包在引号 + 上下文里）也不会独占一行。
fn extract_json_payload(pty_output: &str) -> Option<&str> {
    // 找最后两个"整行等于 marker"的行，记录字节起止 offset，回切原 &str。
    // line_indices 提供每行起始 offset + 内容；行尾 `\r\n` / `\r` / `\n` 都被 lines() 吃掉。
    let mut last: Option<(usize, usize)> = None;
    let mut prev: Option<(usize, usize)> = None;
    let mut cursor = 0;
    for raw_line in pty_output.split('\n') {
        let start = cursor;
        let end = cursor + raw_line.len();
        cursor = end + 1; // 跳过 `\n`
                          // line trim 后等于 marker（兼容 `\r\n` —— raw_line 含末尾 `\r`，trim 掉）
        if raw_line.trim() == JSON_MARKER {
            prev = last;
            last = Some((start, end));
        }
    }
    let (_, prev_end) = prev?;
    let (last_start, _) = last?;
    // prev_end 是前一个 marker 行的结束（不含 `\n`）；下一字符是 `\n`，跳过再取
    let body_start = prev_end + 1;
    if body_start > last_start {
        return None;
    }
    Some(pty_output[body_start..last_start].trim_matches('\n').trim())
}

// ─── 解释器脚本 ───────────────────────────────────────────────────

/// match_file 的 python3 脚本。位置参数：path find before after
///
/// 启动包装器先解码 Base64 参数，脚本拿到的 sys.argv[2] 是原始 UTF-8 字符串。
///
/// **远端 cap = 50 matches**：count 仍记真实总命中数，但 matches 数组只装前 50 个 +
/// `truncated` 标识。理由：后端只在解析后才截断，但前端 PTY 流要在 sentinel 出现前
/// 把全部 output buffer 住 —— 短 find（如 "a"）配大文件 (`/var/log/*`) 可产 MB 级 JSON，
/// 在 backend 拿到 payload 前先把前端 UI OOM。在远端就 cap 才彻底安全。
const PYTHON_MATCH_SCRIPT: &str = r#"import sys,json
M="__RSSH_JSON__"
def o(x):sys.stdout.write(M+"\n"+json.dumps(x,ensure_ascii=True)+"\n"+M+"\n")
p=sys.argv[1];f=sys.argv[2];b=int(sys.argv[3]);a=int(sys.argv[4])
try:t=open(p,"rb").read().decode("utf-8")
except FileNotFoundError:o({"error":"file_not_found"});sys.exit(0)
except Exception as e:o({"error":"io_error","message":str(e)});sys.exit(0)
m=[];c=0;i=0;n=len(f);L=len(t);K=50
while True:
 j=t.find(f,i)
 if j<0:break
 c+=1
 if len(m)<K:
  m.append({"line":t.count("\n",0,j)+1,"context":t[max(0,j-b):min(L,j+n+a)]})
 i=j+n
r={"count":c,"matches":m}
if c>K:r["truncated"]=True;r["matches_shown"]=K
o(r)
"#;

/// patch_file Stage 2 的 python3 脚本（in-place 改 tmp）。位置参数：tmp_path find replace expected
/// 前置：shell 已 `cp path tmp`。脚本读 tmp → 校验 count → in-place 替换 → JSON {count}。
/// count 不匹配时删除 tmp，避免半成品残留。
const PYTHON_PATCH_SCRIPT: &str = r#"import sys,json,os
M="__RSSH_JSON__"
def o(x):sys.stdout.write(M+"\n"+json.dumps(x,ensure_ascii=True)+"\n"+M+"\n")
p=sys.argv[1];f=sys.argv[2];r=sys.argv[3];e=int(sys.argv[4])
try:t=open(p,"rb").read().decode("utf-8")
except Exception as ex:o({"error":"io_error","message":str(ex)});sys.exit(0)
c=t.count(f)
if c!=e:
 try:os.unlink(p)
 except Exception:pass
 o({"error":"count_mismatch","actual":c,"expected":e});sys.exit(0)
try:open(p,"wb").write(t.replace(f,r).encode("utf-8"))
except Exception as ex:o({"error":"tmp_write_failed","message":str(ex)});sys.exit(0)
o({"count":c})
"#;

/// match_file 的 perl 脚本（降级路径）。位置参数：path find before after
/// `\Q...\E` 把 find 当字面量匹配，跳过 regex 元字符。utf8::decode 让长度按字符算。
/// 同 PYTHON_MATCH_SCRIPT：远端 cap = 50 matches，count 仍记真实总数。
const PERL_MATCH_SCRIPT: &str = r#"use strict;use warnings;use JSON::PP;
my $M="__RSSH_JSON__";
sub o{print $M,"\n",JSON::PP->new->ascii->encode($_[0]),"\n",$M,"\n"}
my($p,$f,$b,$a)=@ARGV;
open(my $h,'<:raw',$p)or do{o({error=>"file_not_found"});exit 0};
local $/;my $t=<$h>//'';close $h;
utf8::decode($t)or do{o({error=>"io_error",message=>"File is not valid UTF-8"});exit 0};
my @m;my $c=0;my $i=0;my $n=length($f);my $L=length($t);my $K=50;
while((my $j=index($t,$f,$i))>=0){
 $c++;
 if(scalar(@m)<$K){
  my $line=1+(()=substr($t,0,$j)=~/\n/g);
  my $pre=$j-$b;$pre=0 if $pre<0;my $post=$j+$n+$a;$post=$L if $post>$L;
  push @m,{line=>$line,context=>substr($t,$pre,$post-$pre)};
 }
 $i=$j+$n;
}
my %r=(count=>$c,matches=>\@m);
if($c>$K){$r{truncated}=JSON::PP::true;$r{matches_shown}=$K}
o(\%r);
"#;

/// patch_file Stage 2 的 perl 脚本（降级路径）。位置参数：tmp_path find replace expected
const PERL_PATCH_SCRIPT: &str = r#"use strict;use warnings;use JSON::PP;
my $M="__RSSH_JSON__";
sub o{print $M,"\n",JSON::PP->new->ascii->encode($_[0]),"\n",$M,"\n"}
my($p,$f,$r,$e)=@ARGV;
open(my $h,'<:raw',$p)or do{o({error=>"io_error",message=>"$!"});exit 0};
local $/;my $t=<$h>//'';close $h;
utf8::decode($t)or do{o({error=>"io_error",message=>"File is not valid UTF-8"});exit 0};
my $c=()=$t=~/\Q$f\E/g;
if($c!=$e){unlink($p);o({error=>"count_mismatch",actual=>$c,expected=>$e});exit 0}
my $new=$t;$new=~s/\Q$f\E/$r/g;utf8::encode($new);
open(my $w,'>:raw',$p)or do{o({error=>"tmp_write_failed",message=>"$!"});exit 0};
print $w $new;close $w;o({count=>$c});
"#;

// PowerShell is only a shell adapter: it resolves native applications and
// invokes the same Python/Perl scripts as POSIX. Never resolve the `diff` alias.
const POWERSHELL_APPLICATIONS: &str = r#"
function Rssh-App([string]$name) {
 $app = Get-Command -Name $name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1;
 if ($null -eq $app) { throw ('Application unavailable: ' + $name); };
 return $app.Source;
};
function Rssh-Python {
 foreach ($name in @('python3', 'python')) {
  $app = Get-Command -Name $name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1;
  if ($null -ne $app) {
   try {
    $null = & $app.Source -c 'import sys;sys.exit(sys.version_info[0] != 3)' 2>$null;
    if ($LASTEXITCODE -eq 0) { return $app.Source; };
   } catch {};
  };
 };
 throw 'Python 3 application unavailable';
};
"#;

const POWERSHELL_PATH: &str = r#"
function Rssh-Path([string]$value) {
 if ($value -eq '~') { $value = $HOME; }
 elseif ($value.StartsWith('~/') -or $value.StartsWith('~\')) { $value = Join-Path $HOME $value.Substring(2); }
 return $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($value);
};
"#;

// ─── Interpreter ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interpreter {
    Python3,
    Perl,
}

impl Interpreter {
    fn binary(self) -> &'static str {
        match self {
            Self::Python3 => "python3",
            Self::Perl => "perl",
        }
    }
    fn script_flag(self) -> &'static str {
        match self {
            Self::Python3 => "-c",
            Self::Perl => "-e",
        }
    }
    fn match_script(self) -> &'static str {
        match self {
            Self::Python3 => PYTHON_MATCH_SCRIPT,
            Self::Perl => PERL_MATCH_SCRIPT,
        }
    }
    fn patch_script(self) -> &'static str {
        match self {
            Self::Python3 => PYTHON_PATCH_SCRIPT,
            Self::Perl => PERL_PATCH_SCRIPT,
        }
    }
}

fn select_interpreter(caps: RemoteCapabilities) -> Option<Interpreter> {
    if caps.python3 {
        Some(Interpreter::Python3)
    } else if caps.perl {
        Some(Interpreter::Perl)
    } else {
        None
    }
}

// ─── Quoting & path helpers ──────────────────────────────────────

/// POSIX 单引号转义。所有 rssh 后端拼装的 PTY 命令都走这个，避免路径含
/// 空格/特殊字符时被 shell 错误解析。
///
/// 转义规则：把整个串包在单引号里；输入中的每个单引号替换为 `'\''`
/// （关闭引号 → 转义字面单引号 → 重开引号）。这是 POSIX shell 通用形态，
/// bash/dash/zsh/ash/busybox 都正确解析，且能可靠覆盖所有 shell 元字符
/// （$ ` * ? & | ; < > 等单引号内都按字面）。
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 校验 path 不含会破坏单行命令的字符。
///
/// 必须拒：
/// - `\n` / `\r`：shell_quote 用 POSIX 单引号包，单引号内换行**字面保留**，命令字节流就有
///   真 `\n` byte，重新触发终端多行粘贴问题；POSIX path 走 shell_quote，必须入口直接拒。
/// - 其他 C0 控制字符 (0x00-0x1F) 和 DEL (0x7F)：避免终端转义字符注入。
fn validate_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty() {
        return Err("empty path");
    }
    for c in path.chars() {
        let code = c as u32;
        if c == '\n' || c == '\r' {
            return Err("path contains newline (\\n or \\r) — would break the single-line command");
        }
        if code < 0x20 || code == 0x7F {
            return Err("path contains control character");
        }
    }
    Ok(())
}

/// 远端文件路径准备：处理 `~` / `~/...` 前缀展开，其余走 shell_quote。
///
/// 直接 shell_quote `~/foo` → `'~/foo'`，单引号禁掉 `~` 展开，shell 看到字面波浪号
/// → 文件找不到。这里把 `~/` 替换为 `"$HOME"/`，让 `$HOME` 在双引号里展开，
/// 而 rest 部分仍走单引号保护特殊字符。
///
/// `~user/...` 形态（其他用户的 home）不支持 —— 这种用例罕见，按字面单引号处理
/// （LLM 想用就报"路径不存在"，明确的错误胜过悄悄改写）。
fn prepare_remote_path(path: &str) -> String {
    if path == "~" {
        "\"$HOME\"".to_string()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("\"$HOME\"/{}", shell_quote(rest))
    } else {
        shell_quote(path)
    }
}

// ─── 命令构造 ────────────────────────────────────────────────────

/// Shell quoting is the only difference in interpreter invocation. The loader
/// contains no double quotes (PowerShell 5's legacy native argument handling
/// removes them), and every argument, including the path, is Base64 data.
fn build_script_cmd(shell: ShellKind, interp: Interpreter, script: &str, args: &[&str]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(script);
    let loader = match interp {
        Interpreter::Python3 => format!("import base64,sys,os;sys.argv[1:]=[base64.b64decode(v[1:]).decode('utf-8') for v in sys.argv[1:]];p=sys.argv[1];sys.argv[1]=os.path.expanduser(p) if p=='~' or p.startswith('~/') or (os.name=='nt' and p.startswith('~\\\\')) else p;exec(base64.b64decode('{encoded}'))"),
        Interpreter::Perl => format!("use MIME::Base64;use Encode;@ARGV=map{{decode('UTF-8',decode_base64(substr($_,1)),Encode::FB_CROAK)}}@ARGV;my $home=$^O eq 'MSWin32'?($ENV{{USERPROFILE}}||($ENV{{HOMEDRIVE}}||'').($ENV{{HOMEPATH}}||'')):$ENV{{HOME}};if(defined($home)&&length($home)&&($ARGV[0] eq '~'||$ARGV[0]=~m{{^~/}}||($^O eq 'MSWin32'&&substr($ARGV[0],0,2) eq '~'.chr(92)))){{$home=decode('UTF-8',$home,Encode::FB_CROAK) if $^O ne 'MSWin32'&&!utf8::is_utf8($home);$ARGV[0]=~s{{^~}}{{$home}}}}eval decode_base64('{encoded}');die $@ if $@;"),
    };
    let quote = |value: &str| {
        if shell == ShellKind::Posix {
            shell_quote(value)
        } else {
            powershell_quote(value)
        }
    };
    let args = args
        .iter()
        .map(|arg| {
            quote(&format!(
                "x{}",
                base64::engine::general_purpose::STANDARD.encode(arg)
            ))
        })
        .collect::<Vec<_>>()
        .join(" ");
    let separator = if interp == Interpreter::Perl {
        "-- "
    } else {
        ""
    };
    if shell == ShellKind::Posix {
        return format!(
            "{} {} {} {separator}{args}",
            interp.binary(),
            interp.script_flag(),
            quote(&loader)
        );
    }
    let application = if interp == Interpreter::Python3 {
        "(Rssh-Python)"
    } else {
        "(Rssh-App 'perl')"
    };
    powershell_command( &format!("{POWERSHELL_APPLICATIONS} & {application} {} {} {separator}{args}; $global:LASTEXITCODE = $LASTEXITCODE;", interp.script_flag(), quote(&loader)))
}

fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn powershell_value(value: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(value);
    format!("([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{encoded}')))")
}

fn powershell_command(body: &str) -> String {
    let body = body.lines().map(str::trim).collect::<Vec<_>>().join(" ");
    let command = format!("& {{ $ErrorActionPreference = 'Stop'; $PSNativeCommandUseErrorActionPreference = $false; $global:LASTEXITCODE = 0; try {{ {body} }} catch {{ [Console]::Error.WriteLine($_.Exception.Message); $global:LASTEXITCODE = 2; }} }}");
    command
}

fn cmd_powershell_command(command: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(command);
    format!("powershell.exe -NoProfile -NonInteractive -Command \"[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); & ([scriptblock]::Create([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{encoded}')))); exit $LASTEXITCODE\"")
}

/// Keep cmd.exe out of exit-code evaluation: both the operation and completion
/// marker run in the same PowerShell process. The display command remains
/// executable on its own, without a completion marker.
fn file_command(shell: ShellKind, raw: &str) -> (String, String, String) {
    if shell == ShellKind::Cmd {
        let sentinel = format!("__rssh_done_{}", uuid::Uuid::new_v4().simple());
        let full = format!("{raw}; [Console]::WriteLine('{sentinel}:' + $global:LASTEXITCODE);");
        return (
            cmd_powershell_command(raw),
            sentinel,
            cmd_powershell_command(&full),
        );
    }
    let (sentinel, full) = shell.sentinel_command(raw);
    (raw.into(), sentinel, full)
}

fn probe_command(shell: ShellKind) -> String {
    if shell == ShellKind::Posix {
        return PROBE_CMD.into();
    }
    powershell_command( &format!("{POWERSHELL_APPLICATIONS} $py = 0; try {{ $null = Rssh-Python; $py = 1; }} catch {{}}; $perl = [int]($null -ne (Get-Command -Name perl -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1)); $diff = [int]($null -ne (Get-Command -Name diff -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1)); [Console]::WriteLine(('py3={{0}} perl={{1}} diff={{2}}' -f $py, $perl, $diff)); $global:LASTEXITCODE = 0;"))
}

fn build_match_cmd(
    shell: ShellKind,
    interp: Interpreter,
    path: &str,
    find: &str,
    before: u32,
    after: u32,
) -> String {
    build_script_cmd(
        shell,
        interp,
        interp.match_script(),
        &[path, find, &before.to_string(), &after.to_string()],
    )
}

fn build_modify_cmd(
    shell: ShellKind,
    interp: Interpreter,
    tmp: &str,
    find: &str,
    replace: &str,
    expected: u32,
) -> String {
    build_script_cmd(
        shell,
        interp,
        interp.patch_script(),
        &[tmp, find, replace, &expected.to_string()],
    )
}

fn build_cp_cmd(shell: ShellKind, path: &str, tmp: &str) -> String {
    if shell == ShellKind::Posix {
        return format!(
            "\\cp -- {} {}",
            prepare_remote_path(path),
            prepare_remote_path(tmp)
        );
    }
    powershell_command(&format!(
        "{POWERSHELL_PATH} [IO.File]::Copy((Rssh-Path {}), (Rssh-Path {}), $false);",
        powershell_value(path),
        powershell_value(tmp)
    ))
}

/// Use the same external unified diff on every platform. Exit 1 means changes;
/// exit >= 2 is failure and never advances to final replacement.
fn build_diff_cmd(shell: ShellKind, path: &str, tmp: &str) -> String {
    if shell == ShellKind::Posix {
        return format!(
            "\\diff -u -- {} {}",
            prepare_remote_path(path),
            prepare_remote_path(tmp)
        );
    }
    powershell_command( &format!("{POWERSHELL_PATH} {POWERSHELL_APPLICATIONS} & (Rssh-App 'diff') -u -- (Rssh-Path {}) (Rssh-Path {}); $global:LASTEXITCODE = $LASTEXITCODE;", powershell_value(path), powershell_value(tmp)))
}

fn build_mv_cmd(shell: ShellKind, tmp: &str, path: &str) -> String {
    if shell == ShellKind::Posix {
        return format!(
            "\\mv -- {} {}",
            prepare_remote_path(tmp),
            prepare_remote_path(path)
        );
    }
    powershell_command( &format!("{POWERSHELL_PATH} [IO.File]::Replace((Rssh-Path {}), (Rssh-Path {}), [NullString]::Value);", powershell_value(tmp), powershell_value(path)))
}

// ─── Actor 上的 file_ops 方法 ─────────────────────────────────────

impl Actor {
    async fn file_interpreter(&mut self, needs_diff: bool) -> Result<Interpreter, String> {
        if !self.cfg.shell_kind.supports_file_operations() {
            return Err("File operations are unavailable for this terminal shell.".into());
        }
        let caps = self
            .ensure_remote_caps()
            .await
            .map_err(|error| error.to_string())?;
        let Some(interp) = select_interpreter(caps) else {
            return Err("The target lacks Python 3 or Perl required for file operations.".into());
        };
        if needs_diff && !caps.diff {
            return Err("The target lacks diff required for patch review.".into());
        }
        Ok(interp)
    }

    /// 探测 Python 3 / Perl / diff，只缓存完整可用的工具链。
    /// 缺依赖或执行失败不缓存，不影响后续工具可用性。
    ///
    /// probe 走 `internal_command`（不弹卡片）—— 只读检查程序与 Python 版本，
    /// 没必要让用户为它单独点一次同意。实际的 cp/python/diff/mv 都走 `command_proposed` 弹卡片。
    async fn ensure_remote_caps(&mut self) -> AppResult<RemoteCapabilities> {
        if let Some(c) = self.remote_caps {
            return Ok(c);
        }
        let cmd_id = uuid::Uuid::new_v4().to_string();
        let (cmd, sentinel, full_cmd) =
            file_command(self.cfg.shell_kind, &probe_command(self.cfg.shell_kind));

        self.audit_push(AuditKind::Note {
            message: "file_ops: probing remote capabilities (python3 / perl / diff)".into(),
        });
        self.emit(
            "internal_command",
            json!({
                "id": cmd_id,
                "tool_call_id": cmd_id,
                "cmd": cmd,
                "full_cmd": full_cmd,
                "sentinel": sentinel,
            }),
        );

        let (outcome, ack) = self.wait_command_outcome(&cmd_id).await?;
        let output = match outcome {
            CommandOutcome::Result {
                exit_code,
                output,
                timed_out,
                early_terminated,
            } if exit_code != 0 || timed_out || early_terminated => {
                // A frontend refusal or incomplete probe says nothing about
                // installed tools. Consume the report without poisoning the
                // cache so the next file operation can probe again.
                let redacted = sanitize::redact(&output, &self.cfg.redact_rules);
                let reason = sanitize::truncate(&redacted, self.cfg.max_output_bytes).text;
                Self::complete_action(ack, Ok(()));
                return Err(AppError::other(
                    "caps_probe_aborted",
                    json!({ "reason": reason, "exit_code": exit_code,
                        "timed_out": timed_out, "early_terminated": early_terminated }),
                ));
            }
            CommandOutcome::Result { output, .. } => output,
            CommandOutcome::Rejected { reason } => {
                Self::complete_action(ack, Ok(()));
                return Err(AppError::other(
                    "caps_probe_aborted",
                    json!({ "reason": reason }),
                ));
            }
        };
        let Some(caps) = parse_capabilities(&output) else {
            Self::complete_action(ack, Ok(()));
            return Err(AppError::other(
                "caps_probe_aborted",
                json!({ "reason": "Invalid capability probe output" }),
            ));
        };
        self.audit_push(AuditKind::Note {
            message: format!(
                "file_ops: caps probed — python3={} perl={} diff={}",
                caps.python3, caps.perl, caps.diff
            ),
        });
        // Missing tools may become available later. Cache a complete usable
        // toolchain only, never a failed or partially available probe.
        if select_interpreter(caps).is_some() && caps.diff {
            self.remote_caps = Some(caps);
        }
        // The internal probe has no terminal card mutation. Its processing ack
        // means the output was consumed into the actor's capability state.
        Self::complete_action(ack, Ok(()));
        Ok(caps)
    }

    /// 跑一条 file_ops 命令：弹 `<event>_proposed` 卡片 → 等用户审批 / 执行 → 审计 + 返回结果。
    ///
    /// 跟 `handle_run_command` 的区别：
    /// 1. **不走 `sanitize::validate`**。cp / mv / python3 等会被 DESTRUCTIVE / INTERPRETERS_DENIED
    ///    拒掉，但 file_ops 的命令是 rssh 后端构造的固定模板（不是 LLM 直传），由 rssh 自己负责安全。
    /// 2. 返回原始 `CommandOutcome` 给上层做错误分支（多步流程要根据 exit / JSON marker
    ///    决定是否中断、是否提示 tmp 残留等）。
    ///
    /// `domain` 是该步骤的领域字段（调用方构造）—— patch 步骤带 step / path / find / replace /
    /// expected_count / diff 等结构化字段；match_file 带 explain / side_effect / kind。本方法只负责
    /// 注入执行信封（id / cmd / execution），不关心领域语义。`event` 决定 emit 的事件名前缀
    /// （"command" 给 match_file，"patch" 给 patch×4），让每种工具拥有自己的 proposal/result 流，
    /// 而共用同一条 PTY 执行 + sentinel + outcome-wait 核。
    async fn run_file_op(
        &mut self,
        cmd: String,
        timeout_s: u32,
        domain: serde_json::Map<String, serde_json::Value>,
        event: &str,
    ) -> AppResult<CommandOutcome> {
        let cmd_id = uuid::Uuid::new_v4().to_string();
        let (cmd, sentinel, full_cmd) = file_command(self.cfg.shell_kind, &cmd);

        // Audit reads explain/side_effect from the domain when the caller
        // carries them (match_file). Patch steps carry structured fields
        // instead, so the audit surfaces just the command — the cmd itself is
        // the meaningful record, and handle_patch_file audits a step-aware Note.
        let explain = domain.get("explain").and_then(|v| v.as_str()).unwrap_or("");
        let side_effect = domain
            .get("side_effect")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        self.audit_push(AuditKind::CommandProposed {
            id: cmd_id.clone(),
            cmd: cmd.clone(),
            explain: explain.to_string(),
            side_effect: side_effect.to_string(),
        });
        let started_at = std::time::Instant::now();

        let mut payload = domain;
        payload.insert("id".into(), json!(cmd_id));
        payload.insert("tool_call_id".into(), json!(cmd_id));
        payload.insert("cmd".into(), json!(cmd));
        payload.insert(
            "execution".into(),
            json!({
                "full_cmd": full_cmd,
                "sentinel": sentinel,
                "timeout_s": timeout_s,
            }),
        );
        self.emit(
            &format!("{event}_proposed"),
            serde_json::Value::Object(payload),
        );

        let (outcome, ack) = self.wait_command_outcome(&cmd_id).await?;
        match outcome {
            CommandOutcome::Rejected { reason } => {
                self.record_rejection(&cmd_id, &reason);
                Self::complete_action(ack, Ok(()));
                Ok(CommandOutcome::Rejected { reason })
            }
            CommandOutcome::Result {
                exit_code,
                output,
                timed_out,
                early_terminated,
            } => {
                let redacted = sanitize::redact(&output, &self.cfg.redact_rules);
                let trunc = sanitize::truncate(&redacted, self.cfg.max_output_bytes);
                self.emit(
                    &format!("{event}_completed"),
                    json!({
                        "id": cmd_id,
                        "exit_code": exit_code,
                        "timed_out": timed_out,
                        "early_terminated": early_terminated,
                        "output": trunc.text,
                        "original_bytes": trunc.original_bytes,
                        "truncated_bytes": trunc.truncated_bytes,
                        "duration_ms": started_at.elapsed().as_millis() as u64,
                    }),
                );
                self.audit_push(AuditKind::CommandExecuted {
                    id: cmd_id,
                    exit_code,
                    output_redacted: trunc.text.clone(),
                    original_bytes: trunc.original_bytes,
                    truncated_bytes: trunc.truncated_bytes,
                    duration_ms: started_at.elapsed().as_millis() as u64,
                });
                // Ack only after the canonical terminal mutation and audit
                // entry exist. Prepare-stop may cancel this handler as soon as
                // the frontend sees the ack.
                Self::complete_action(ack, Ok(()));
                // 返回 redact 前的 raw output —— 上层要从中 extract_json_payload，
                // 脱敏后的版本里 marker 可能被改写。脱敏只作用于 audit / UI 显示，
                // 不影响 rssh 内部解析。
                Ok(CommandOutcome::Result {
                    exit_code,
                    output,
                    timed_out,
                    early_terminated,
                })
            }
        }
    }

    pub(super) async fn handle_match_file(&mut self, tc: ToolCall) -> AppResult<ChatMessage> {
        let input: MatchFileInput = match serde_json::from_value(tc.input.clone()) {
            Ok(i) => i,
            Err(e) => {
                return Ok(self.make_tool_error(&tc.id, &format!("Failed to parse input: {e}")))
            }
        };
        if let Err(e) = validate_path(&input.path) {
            return Ok(self.make_tool_error(&tc.id, &format!("match_file: invalid path — {e}")));
        }
        if input.find.is_empty() {
            return Ok(self.make_tool_error(
                &tc.id,
                "match_file: `find` must not be empty (it would match the entire file).",
            ));
        }
        let before = input
            .before
            .unwrap_or(MATCH_CONTEXT_DEFAULT)
            .min(MATCH_CONTEXT_MAX);
        let after = input
            .after
            .unwrap_or(MATCH_CONTEXT_DEFAULT)
            .min(MATCH_CONTEXT_MAX);

        let interp = match self.file_interpreter(false).await {
            Ok(interp) => interp,
            Err(reason) => return Ok(self.make_tool_error(&tc.id, &reason)),
        };

        let cmd = build_match_cmd(
            self.cfg.shell_kind,
            interp,
            &input.path,
            &input.find,
            before,
            after,
        );
        let domain = serde_json::Map::from_iter([
            ("path".into(), json!(&input.path)),
            ("find".into(), json!(&input.find)),
            ("before".into(), json!(before)),
            ("after".into(), json!(after)),
        ]);
        let outcome = self.run_file_op(cmd, 60, domain, "match").await?;

        let (exit_code, output) = match outcome {
            CommandOutcome::Rejected { reason } => {
                return Ok(self.make_tool_error(
                    &tc.id,
                    &format!("User rejected match_file. Reason: {reason}."),
                ));
            }
            CommandOutcome::Result {
                exit_code, output, ..
            } => (exit_code, output),
        };

        let payload = match (exit_code, extract_json_payload(&output)) {
            (0, Some(p)) => p.to_string(),
            _ => {
                return Ok(self.make_tool_error(
                    &tc.id,
                    &format!(
                        "match_file: remote script failed (exit {exit_code}). Output: {}",
                        output.chars().take(400).collect::<String>()
                    ),
                ));
            }
        };
        // Remote returns `{"error": ...}` for file_not_found / io_error —
        // bubble those as tool errors so the LLM doesn't treat count=0 as
        // "no matches" and barrel into a patch_file that also fails.
        let parsed: serde_json::Value = match serde_json::from_str(&payload) {
            Ok(v) => v,
            Err(e) => {
                return Ok(self.make_tool_error(
                    &tc.id,
                    &format!("match_file: malformed JSON ({e}). Raw: {payload}"),
                ));
            }
        };
        if let Some(err) = parsed.get("error").and_then(|e| e.as_str()) {
            let msg = match err {
                "file_not_found" => format!("match_file: file not found: {}", input.path),
                "io_error" => format!(
                    "match_file: io_error reading {} ({})",
                    input.path,
                    parsed.get("message").and_then(|m| m.as_str()).unwrap_or("")
                ),
                other => format!("match_file: remote error {other}: {payload}"),
            };
            return Ok(self.make_tool_error(&tc.id, &msg));
        }
        let count = parsed.get("count").and_then(|c| c.as_u64());
        let matches_shown = parsed
            .get("matches")
            .and_then(|m| m.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let remote_truncated = parsed
            .get("truncated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Remote script caps `matches` at K=50; we forward as-is.
        self.audit_push(AuditKind::Note {
            message: format!(
                "match_file: {} interp={} count={} matches_shown={}{}",
                input.path,
                interp.binary(),
                count.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
                matches_shown,
                if remote_truncated {
                    " (remote-truncated)"
                } else {
                    ""
                }
            ),
        });
        // Redact at the insertion boundary so `redact_message` in dialogue_turn
        // can skip a second pass on the JSON payload (which would corrupt
        // structured hashes — sha256/git-oid — inside string fields).
        let redacted_payload = sanitize::redact(&payload, &self.cfg.redact_rules);
        Ok(Self::make_tool_result(&tc.id, redacted_payload, true))
    }

    pub(super) async fn handle_patch_file(&mut self, tc: ToolCall) -> AppResult<ChatMessage> {
        let input: PatchFileInput = match serde_json::from_value(tc.input.clone()) {
            Ok(i) => i,
            Err(e) => {
                return Ok(self.make_tool_error(&tc.id, &format!("Failed to parse input: {e}")))
            }
        };
        if let Err(e) = validate_path(&input.path) {
            return Ok(self.make_tool_error(&tc.id, &format!("patch_file: invalid path — {e}")));
        }
        if input.find.is_empty() {
            return Ok(self.make_tool_error(&tc.id, "patch_file: `find` must not be empty."));
        }
        if input.expected_count == 0 {
            return Ok(self.make_tool_error(&tc.id, "patch_file: `expected_count` must be >= 1."));
        }

        let interp = match self.file_interpreter(true).await {
            Ok(interp) => interp,
            Err(reason) => return Ok(self.make_tool_error(&tc.id, &reason)),
        };

        // tmp 与 path 同目录，保证后续 mv 走单 rename(2)（同 filesystem，原子）。
        let tmp_suffix: String = uuid::Uuid::new_v4()
            .simple()
            .to_string()
            .chars()
            .take(8)
            .collect();
        let tmp_path = format!("{}.rssh-{}", input.path, tmp_suffix);

        // ── Card 1/4: cp 原文到 tmp ──────────────────────────────
        let domain = serde_json::Map::from_iter([
            ("step".into(), json!("cp")),
            ("path".into(), json!(&input.path)),
            ("tmp_path".into(), json!(&tmp_path)),
        ]);
        let outcome = self
            .run_file_op(
                build_cp_cmd(self.cfg.shell_kind, &input.path, &tmp_path),
                30,
                domain,
                "patch",
            )
            .await?;
        match outcome {
            CommandOutcome::Rejected { reason } => {
                return Ok(self.make_tool_error(
                    &tc.id,
                    &format!("User rejected step 1/4 (cp). Reason: {reason}."),
                ));
            }
            CommandOutcome::Result {
                exit_code, output, ..
            } if exit_code != 0 => {
                return Ok(self.make_tool_error(
                    &tc.id,
                    &format!(
                        "patch_file 1/4 (cp) failed (exit {exit_code}). Output: {}",
                        output.chars().take(400).collect::<String>()
                    ),
                ));
            }
            _ => {}
        }

        // ── Card 2/4: 解释器 in-place 改 tmp（校验 count → 替换 → 回 {count}） ──
        let domain = serde_json::Map::from_iter([
            ("step".into(), json!("modify")),
            ("path".into(), json!(&input.path)),
            ("tmp_path".into(), json!(&tmp_path)),
            ("find".into(), json!(&input.find)),
            ("replace".into(), json!(&input.replace)),
            ("expected_count".into(), json!(input.expected_count)),
        ]);
        let outcome = self
            .run_file_op(
                build_modify_cmd(
                    self.cfg.shell_kind,
                    interp,
                    &tmp_path,
                    &input.find,
                    &input.replace,
                    input.expected_count,
                ),
                60,
                domain,
                "patch",
            )
            .await?;
        let (modify_exit, modify_output) = match outcome {
            CommandOutcome::Rejected { reason } => {
                return Ok(self.make_tool_error(
                    &tc.id,
                    &format!(
                        "User rejected step 2/4 (modify). Reason: {reason}. Tmp at {tmp_path} (staged file retained)."
                    ),
                ));
            }
            CommandOutcome::Result {
                exit_code, output, ..
            } => (exit_code, output),
        };
        if modify_exit != 0 {
            return Ok(self.make_tool_error(
                &tc.id,
                &format!(
                    "patch_file 2/4 (modify) failed (exit {modify_exit}). Tmp at {tmp_path}. Output: {}",
                    modify_output.chars().take(400).collect::<String>()
                ),
            ));
        }
        let payload = match extract_json_payload(&modify_output) {
            Some(p) => p.to_string(),
            None => {
                return Ok(self.make_tool_error(
                    &tc.id,
                    &format!(
                        "patch_file 2/4 (modify): no JSON marker in output. Tmp at {tmp_path}. Output: {}",
                        modify_output.chars().take(400).collect::<String>()
                    ),
                ));
            }
        };
        let parsed: serde_json::Value = match serde_json::from_str(&payload) {
            Ok(v) => v,
            Err(e) => {
                return Ok(self.make_tool_error(
                    &tc.id,
                    &format!("patch_file 2/4 (modify): malformed JSON ({e}). Tmp at {tmp_path}. Raw: {payload}"),
                ));
            }
        };
        if let Some(err) = parsed.get("error").and_then(|e| e.as_str()) {
            let msg = match err {
                "count_mismatch" => format!(
                    "patch_file: count mismatch — file has {} occurrence(s), expected {}.",
                    parsed.get("actual").and_then(|a| a.as_u64()).unwrap_or(0),
                    parsed.get("expected").and_then(|a| a.as_u64()).unwrap_or(0),
                ),
                "io_error" | "tmp_write_failed" => format!(
                    "patch_file: remote {} ({}). Tmp may exist at {tmp_path}.",
                    err,
                    parsed.get("message").and_then(|m| m.as_str()).unwrap_or("")
                ),
                other => format!("patch_file: remote error {other}: {payload}"),
            };
            return Ok(self.make_tool_error(&tc.id, &msg));
        }
        let count = parsed
            .get("count")
            .and_then(|c| c.as_u64())
            .unwrap_or(input.expected_count as u64) as u32;

        // ── Card 3/4: diff -u path tmp（输出展示给用户）─────────
        // exit 0=无差异 / 1=有差异（正常）/ >=2=工具失败
        let domain = serde_json::Map::from_iter([
            ("step".into(), json!("diff")),
            ("path".into(), json!(&input.path)),
            ("tmp_path".into(), json!(&tmp_path)),
        ]);
        let outcome = self
            .run_file_op(
                build_diff_cmd(self.cfg.shell_kind, &input.path, &tmp_path),
                30,
                domain,
                "patch",
            )
            .await?;
        let (diff_exit, diff_raw) = match outcome {
            CommandOutcome::Rejected { reason } => {
                return Ok(self.make_tool_error(
                    &tc.id,
                    &format!(
                        "User rejected step 3/4 (diff). Reason: {reason}. Tmp at {tmp_path} (staged file retained)."
                    ),
                ));
            }
            CommandOutcome::Result {
                exit_code,
                output,
                timed_out,
                early_terminated,
            } => {
                if !(0..=1).contains(&exit_code) || timed_out || early_terminated {
                    return Ok(self.make_tool_error(
                        &tc.id,
                        &format!(
                            "patch_file 3/4 (diff) failed or was incomplete (exit {exit_code}). Tmp at {tmp_path}. Output: {}",
                            output.chars().take(400).collect::<String>()
                        ),
                    ));
                }
                (exit_code, output)
            }
        };

        // diff exit 0 = modify step was a no-op (replacement equals original).
        // Skip mv so we don't churn mtime/inode for tools (make / caches / backups)
        // that key off them. Leave tmp for the user to inspect / rm.
        if diff_exit == 0 {
            self.audit_push(AuditKind::Note {
                message: format!(
                    "patch_file no-op: {} matched count={} but replacement equals original (interp={})",
                    input.path, count, interp.binary()
                ),
            });
            let result = json!({
                "diff": "",
                "diff_truncated_bytes": 0,
                "changed": 0,
                "no_op": true,
                "note": format!("patch_file: no-op (replacement matches original; mv skipped). Tmp at {tmp_path}, staged file retained."),
            })
            .to_string();
            // Redact at insertion boundary, mark pre_redacted so redact_message
            // doesn't run a second pass on this structured JSON.
            let redacted = sanitize::redact(&result, &self.cfg.redact_rules);
            return Ok(Self::make_tool_result(&tc.id, redacted, true));
        }

        // diff 走 max_output_bytes 截断 —— 原始 diff 可能很长（大文件差异、二进制差异等），
        // 不截断会同时撑爆 (a) mv 卡片的 emit payload，导致前端 UI 渲染卡顿；(b) ToolResult
        // 进 LLM history，吃掉 context window。截断后保留前缀供人审 / LLM 概览，丢失尾部
        // 由 `diff_truncated_bytes` 显式告知。
        let diff_trunc = sanitize::truncate(&diff_raw, self.cfg.max_output_bytes);
        if diff_trunc.truncated_bytes > 0 {
            return Ok(self.make_tool_error(
                &tc.id,
                &format!("patch_file: diff exceeds the complete-review limit; original file unchanged. Staged file retained at {tmp_path}."),
            ));
        }
        let diff = diff_trunc.text;
        let diff_truncated_bytes = diff_trunc.truncated_bytes;

        // ── Card 4/4: mv tmp → path（原子覆盖） ─────────────────
        let domain = serde_json::Map::from_iter([
            ("step".into(), json!("mv")),
            ("path".into(), json!(&input.path)),
            ("tmp_path".into(), json!(&tmp_path)),
            ("diff".into(), json!(&diff)),
        ]);
        let outcome = self
            .run_file_op(
                build_mv_cmd(self.cfg.shell_kind, &tmp_path, &input.path),
                30,
                domain,
                "patch",
            )
            .await?;
        match outcome {
            CommandOutcome::Rejected { reason } => {
                Ok(self.make_tool_error(
                    &tc.id,
                    &format!(
                        "User rejected step 4/4 (mv). Reason: {reason}. Tmp at {tmp_path} (staged file retained)."
                    ),
                ))
            }
            CommandOutcome::Result {
                exit_code, output, ..
            } if exit_code != 0 => {
                Ok(self.make_tool_error(
                    &tc.id,
                    &format!(
                        "patch_file 4/4 (mv) failed (exit {exit_code}). Tmp at {tmp_path}. Output: {}",
                        output.chars().take(400).collect::<String>()
                    ),
                ))
            }
            CommandOutcome::Result { .. } => {
                self.audit_push(AuditKind::Note {
                    message: format!(
                        "patch_file: {} interp={} changed={}",
                        input.path,
                        interp.binary(),
                        count
                    ),
                });
                let result = json!({
                    "diff": diff,
                    "diff_truncated_bytes": diff_truncated_bytes,
                    "changed": count,
                })
                .to_string();
                // Redact at insertion boundary, mark pre_redacted to skip
                // the second pass that would substitute `<REDACTED:hex>`
                // into sha256/git-oid strings embedded in the diff.
                let redacted = sanitize::redact(&result, &self.cfg.redact_rules);
                Ok(Self::make_tool_result(&tc.id, redacted, true))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::session::{PendingSession, SessionConfig, UserAction};
    use tokio::sync::{mpsc, oneshot};

    type ProbeEvents = mpsc::UnboundedReceiver<(String, serde_json::Value)>;

    fn powershell_available() -> bool {
        std::process::Command::new("pwsh")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$PSVersionTable.PSVersion",
            ])
            .output()
            .is_ok_and(|result| result.status.success())
    }

    fn run_powershell(cmd: &str) -> (i32, String) {
        assert!(
            !cmd.contains(['\n', '\r']),
            "PTY commands must stay on one line"
        );
        // Exercise the exact PowerShell payload invoked from cmd.exe. The test
        // host need not be Windows; cmd.exe's own parser needs Windows coverage.
        if let Some(payload) =
            cmd.strip_prefix("powershell.exe -NoProfile -NonInteractive -Command \"")
        {
            let payload = payload.strip_suffix('"').unwrap();
            assert!(
                !payload.contains('"'),
                "cmd.exe wrapper must contain only the fixed decoder and Base64"
            );
            let output = std::process::Command::new("pwsh")
                .args(["-NoProfile", "-NonInteractive", "-Command", &format!("$global:LASTEXITCODE=97; $PSNativeCommandArgumentPassing='Legacy'; $env:PYTHONIOENCODING='ascii'; {payload}")])
                .output()
                .unwrap();
            return (
                output.status.code().unwrap(),
                String::from_utf8(output.stdout).unwrap(),
            );
        }
        let wrapped = crate::ai::shell::ShellKind::Powershell.format_sentinel(
            &format!(
                "$global:LASTEXITCODE=97; $PSNativeCommandArgumentPassing='Legacy'; $env:PYTHONIOENCODING='ascii'; {cmd}"
            ),
            "__test_done",
        );
        let output = std::process::Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-Command", &wrapped])
            .output()
            .unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        let (body, code) = text.rsplit_once("__test_done:").unwrap_or_else(|| {
            panic!(
                "missing result: {text}; stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (code.trim().parse().unwrap(), body.to_string())
    }

    #[cfg(not(windows))]
    fn apply_unified_diff(path: &std::path::Path, diff: &str) {
        use std::io::Write;
        let mut patch = std::process::Command::new("patch")
            .arg("--batch")
            .arg(path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        patch
            .stdin
            .take()
            .unwrap()
            .write_all(diff.as_bytes())
            .unwrap();
        assert!(
            patch.wait().unwrap().success(),
            "review must be an applicable unified diff"
        );
    }

    #[test]
    fn powershell_uses_shared_scripts_for_literal_unicode_crlf_and_empty_replacement() {
        if !powershell_available() {
            return;
        }
        for shell in [ShellKind::Powershell, ShellKind::Cmd] {
            let run = |command: &str| run_powershell(&file_command(shell, command).0);
            for interp in [Interpreter::Python3, Interpreter::Perl] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("it's [literal] 中文.txt");
                let tmp = dir.path().join("it's [literal] 中文.txt.tmp");
                let original =
                    "\u{feff}header\r\n旧 ' \" $HOME `x` $(throw 'injected') 🦀\r\nnext\r\n";
                let find = "旧 ' \" $HOME `x` $(throw 'injected') 🦀\r\nnext";
                let replacement = "新 ' \" $HOME `x` 🦀\r\nline two";
                std::fs::write(&path, original).unwrap();
                let path = path.to_str().unwrap();
                let tmp = tmp.to_str().unwrap();
                let (code, output) = run(&build_match_cmd(shell, interp, path, find, 80, 80));
                assert_eq!(code, 0, "{output}");
                assert!(
                    output.is_ascii(),
                    "JSON must not depend on a Windows codepage"
                );
                let value: serde_json::Value =
                    serde_json::from_str(extract_json_payload(&output).unwrap()).unwrap();
                assert_eq!(value["count"], 1, "{output}");
                assert_eq!(value["matches"][0]["context"], original);
                assert_eq!(run(&build_cp_cmd(shell, path, tmp)).0, 0);
                let (code, output) =
                    run(&build_modify_cmd(shell, interp, tmp, find, replacement, 1));
                assert_eq!(code, 0, "{output}");
                assert_eq!(std::fs::read_to_string(path).unwrap(), original);
                assert_eq!(
                    std::fs::read_to_string(tmp).unwrap(),
                    original.replace(find, replacement)
                );
                let (code, diff) = run(&build_diff_cmd(shell, path, tmp));
                assert_eq!(code, 1, "{diff}");
                assert!(diff.contains("-旧") && diff.contains("+新"), "{diff}");
                #[cfg(not(windows))]
                {
                    let review = dir.path().join("review.txt");
                    std::fs::write(&review, original).unwrap();
                    apply_unified_diff(&review, &diff);
                    assert_eq!(
                        std::fs::read_to_string(review).unwrap(),
                        original.replace(find, replacement)
                    );
                }
                assert_eq!(run(&build_mv_cmd(shell, tmp, path)).0, 0);
                let (code, output) =
                    run(&build_modify_cmd(shell, interp, path, replacement, "", 1));
                assert_eq!(code, 0, "{output}");
                assert_eq!(
                    std::fs::read_to_string(path).unwrap(),
                    original.replace(find, "")
                );
            }
        }
    }

    #[test]
    fn shared_scripts_keep_originals_on_count_mismatch_and_invalid_utf8() {
        if !powershell_available() {
            return;
        }
        for interp in [Interpreter::Python3, Interpreter::Perl] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("source.txt");
            let tmp = dir.path().join("source.tmp");
            let original = "\u{feff}旧\r\n旧";
            std::fs::write(&path, original).unwrap();
            let path = path.to_str().unwrap();
            let tmp = tmp.to_str().unwrap();
            assert_eq!(
                run_powershell(&build_cp_cmd(ShellKind::Powershell, path, tmp)).0,
                0
            );
            let (_, output) = run_powershell(&build_modify_cmd(
                ShellKind::Powershell,
                interp,
                tmp,
                "旧",
                "新",
                1,
            ));
            let value: serde_json::Value =
                serde_json::from_str(extract_json_payload(&output).unwrap()).unwrap();
            assert_eq!(value["error"], "count_mismatch", "{output}");
            assert_eq!(std::fs::read_to_string(path).unwrap(), original);
            assert!(!std::path::Path::new(tmp).exists());
            let invalid = [0xff, b'o', b'l', b'd'];
            std::fs::write(tmp, invalid).unwrap();
            let (_, output) = run_powershell(&build_modify_cmd(
                ShellKind::Powershell,
                interp,
                tmp,
                "old",
                "new",
                1,
            ));
            let value: serde_json::Value =
                serde_json::from_str(extract_json_payload(&output).unwrap()).unwrap();
            assert_eq!(value["error"], "io_error", "{output}");
            assert_eq!(std::fs::read(tmp).unwrap(), invalid);
        }
    }

    #[test]
    #[cfg(not(windows))]
    fn posix_file_scripts_run_in_sh_without_ansi_c_quoting() {
        for shell in ["sh", "dash"] {
            if std::process::Command::new(shell)
                .args(["-c", "exit 0"])
                .status()
                .is_err()
            {
                continue;
            }
            let run = |cmd: &str| {
                assert!(!cmd.contains(['\n', '\r']));
                assert!(!cmd.contains("$'"));
                let output = std::process::Command::new(shell)
                    .args(["-c", cmd])
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                String::from_utf8(output.stdout).unwrap()
            };
            for interp in [Interpreter::Python3, Interpreter::Perl] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("it's [字].txt");
                let original = "\u{feff}  old ' \" $HOME `no` 🦀\r\n    next";
                let replacement = "  新\r\n    '$()' \" `no`";
                std::fs::write(&path, original).unwrap();
                let path = path.to_str().unwrap();
                let output = run(&build_match_cmd(
                    ShellKind::Posix,
                    interp,
                    path,
                    original,
                    80,
                    80,
                ));
                let value: serde_json::Value =
                    serde_json::from_str(extract_json_payload(&output).unwrap()).unwrap();
                assert_eq!(value["count"], 1);
                assert_eq!(value["matches"][0]["context"], original);
                run(&build_modify_cmd(
                    ShellKind::Posix,
                    interp,
                    path,
                    original,
                    replacement,
                    1,
                ));
                assert_eq!(std::fs::read_to_string(path).unwrap(), replacement);
                run(&build_modify_cmd(
                    ShellKind::Posix,
                    interp,
                    path,
                    replacement,
                    "",
                    1,
                ));
                assert_eq!(std::fs::read(path).unwrap(), b"");
            }
        }
    }

    #[test]
    fn powershell_probe_and_diff_ignore_aliases() {
        if !powershell_available() {
            return;
        }
        let (code, output) = run_powershell(&format!(
            "Set-Alias diff Write-Output; {}",
            probe_command(ShellKind::Powershell)
        ));
        assert_eq!(code, 0, "{output}");
        let caps = parse_capabilities(&output).unwrap();
        assert!(caps.python3 && caps.perl && caps.diff, "{output}");
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        let new = dir.path().join("new");
        std::fs::write(&old, "old\n").unwrap();
        std::fs::write(&new, "new\n").unwrap();
        let (code, diff) = run_powershell(&format!(
            "Set-Alias diff Write-Output; {}",
            build_diff_cmd(
                ShellKind::Powershell,
                old.to_str().unwrap(),
                new.to_str().unwrap()
            )
        ));
        assert_eq!(code, 1, "{diff}");
        assert!(diff.contains("-old\n+new"), "{diff}");
    }

    #[cfg(unix)]
    #[test]
    fn powershell_python3_probe_falls_back_to_validated_python_application() {
        use std::os::unix::fs::PermissionsExt;
        if !powershell_available() {
            return;
        }
        let python = std::process::Command::new("python3")
            .args(["-c", "import sys;print(sys.executable)"])
            .output()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("python3");
        std::fs::write(
            &fake,
            "#!/bin/sh\nprintf 'not Python 3\\n'\nprintf 'unavailable\\n' >&2\nexit 42\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(
            String::from_utf8(python.stdout).unwrap().trim(),
            dir.path().join("python"),
        )
        .unwrap();
        let setup = format!(
            "$env:PATH = {};",
            powershell_value(dir.path().to_str().unwrap())
        );
        let (code, output) =
            run_powershell(&format!("{setup} {}", probe_command(ShellKind::Powershell)));
        assert_eq!(code, 0, "{output}");
        assert_eq!(
            parse_capabilities(&output),
            Some(RemoteCapabilities {
                python3: true,
                perl: false,
                diff: false
            })
        );
        assert!(
            !output.contains("not Python"),
            "candidate stdout must not become a command name"
        );
        let path = dir.path().join("中文.txt");
        std::fs::write(&path, "old").unwrap();
        let (code, output) = run_powershell(&format!(
            "{setup} {}",
            build_match_cmd(
                ShellKind::Powershell,
                Interpreter::Python3,
                path.to_str().unwrap(),
                "old",
                0,
                0
            )
        ));
        assert_eq!(code, 0, "{output}");
        let value: serde_json::Value =
            serde_json::from_str(extract_json_payload(&output).unwrap()).unwrap();
        assert_eq!(value["count"], 1, "{output}");
    }

    #[cfg(unix)]
    #[test]
    fn shared_scripts_expand_unicode_home_without_double_encoding() {
        if !powershell_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("用户");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join("中文.txt"), "找到\r\n").unwrap();
        for interp in [Interpreter::Python3, Interpreter::Perl] {
            let cmd = build_match_cmd(ShellKind::Powershell, interp, "~/中文.txt", "找到", 0, 0);
            let (code, output) = run_powershell(&format!(
                "$env:HOME = {}; {cmd}",
                powershell_value(home.to_str().unwrap())
            ));
            assert_eq!(code, 0, "{output}");
            let value: serde_json::Value =
                serde_json::from_str(extract_json_payload(&output).unwrap()).unwrap();
            assert_eq!(value["count"], 1, "{output}");
            let output = std::process::Command::new("sh")
                .env("HOME", &home)
                .args([
                    "-c",
                    &build_match_cmd(ShellKind::Posix, interp, "~/中文.txt", "找到", 0, 0),
                ])
                .output()
                .unwrap();
            let output = String::from_utf8(output.stdout).unwrap();
            let value: serde_json::Value =
                serde_json::from_str(extract_json_payload(&output).unwrap()).unwrap();
            assert_eq!(value["count"], 1, "{output}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn perl_absolute_path_does_not_decode_unrelated_home_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, "old").unwrap();
        let output = std::process::Command::new("sh")
            .env("HOME", std::ffi::OsString::from_vec(b"/tmp/\xe9".to_vec()))
            .args([
                "-c",
                &build_match_cmd(
                    ShellKind::Posix,
                    Interpreter::Perl,
                    path.to_str().unwrap(),
                    "old",
                    0,
                    0,
                ),
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(extract_json_payload(&output).unwrap()).unwrap();
        assert_eq!(value["count"], 1, "{output}");
    }

    #[test]
    fn windows_completion_marker_preserves_success_diff_and_error_codes() {
        if !powershell_available() {
            return;
        }
        for shell in [ShellKind::Powershell, ShellKind::Cmd] {
            for expected in [0, 1, 2] {
                let raw = powershell_command(&format!("$global:LASTEXITCODE = {expected};"));
                let (display, sentinel, full) = file_command(shell, &raw);
                if shell == ShellKind::Cmd {
                    assert!(display.starts_with("powershell.exe "));
                    assert!(full.starts_with("powershell.exe "));
                    assert!(!full.contains("%errorlevel%"));
                }
                let (code, output) = run_powershell(&full);
                assert_eq!(code, expected, "{output}");
                assert_eq!(output.trim(), format!("{sentinel}:{expected}"));
            }
        }
    }

    #[test]
    fn powershell_missing_diff_is_an_error_not_a_change() {
        if !powershell_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let command = build_diff_cmd(ShellKind::Powershell, "old.txt", "new.txt");
        let (code, _) = run_powershell(&format!(
            "$env:PATH = {}; {command}",
            powershell_value(dir.path().to_str().unwrap())
        ));
        assert!(
            code >= 2,
            "missing diff must not look like a valid diff (exit {code})"
        );
    }

    #[tokio::test]
    async fn patch_never_proposes_replace_after_incomplete_or_oversized_diff() {
        for (exit_code, output, timed_out, early_terminated) in [
            (1, "x".repeat(65), false, false),
            (1, "partial".into(), true, false),
            (1, "partial".into(), false, true),
            (-1, "failed".into(), false, false),
            (2, "diff unavailable".into(), false, false),
        ] {
            let (mut pending, mut events) = probe_session();
            pending.actor.cfg.shell_kind = ShellKind::Powershell;
            pending.actor.remote_caps = Some(RemoteCapabilities {
                python3: true,
                perl: false,
                diff: true,
            });
            pending.actor.cfg.max_output_bytes = 64;
            let sender = pending.info().action_tx.clone();
            let respond = async {
                for (step, code, body, timeout, early) in [
                    ("cp", 0, String::new(), false, false),
                    (
                        "modify",
                        0,
                        "__RSSH_JSON__\n{\"count\":1}\n__RSSH_JSON__\n".into(),
                        false,
                        false,
                    ),
                    ("diff", exit_code, output, timed_out, early_terminated),
                ] {
                    let payload = loop {
                        let (event, payload) = events.recv().await.unwrap();
                        if event == "ai:patch_proposed:caps-test" {
                            break payload;
                        }
                    };
                    assert_eq!(payload["step"], step);
                    sender
                        .send(UserAction::CommandResult {
                            command_id: payload["id"].as_str().unwrap().into(),
                            exit_code: code,
                            output: body,
                            timed_out: timeout,
                            early_terminated: early,
                            ack: None,
                        })
                        .unwrap();
                }
            };
            let call = ToolCall {
                id: "patch".into(),
                name: "patch_file".into(),
                input: json!({"path": "test.txt", "find": "old", "replace": "new", "expected_count": 1}),
            };
            let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                tokio::join!(pending.actor.handle_patch_file(call), respond)
            })
            .await
            .expect("incomplete review must return without another approval");
            assert!(matches!(
                result.unwrap(),
                ChatMessage::ToolResult { is_error: true, .. }
            ));
            while let Ok((event, payload)) = events.try_recv() {
                assert!(event != "ai:patch_proposed:caps-test" || payload["step"] != "mv");
            }
        }
    }

    #[tokio::test]
    async fn windows_file_operations_use_the_shared_interpreters() {
        let (mut pending, mut events) = probe_session();
        for shell in [ShellKind::Powershell, ShellKind::Cmd] {
            pending.actor.cfg.shell_kind = shell;
            for (python3, perl, expected) in [
                (true, true, Interpreter::Python3),
                (false, true, Interpreter::Perl),
            ] {
                pending.actor.remote_caps = Some(RemoteCapabilities {
                    python3,
                    perl,
                    diff: true,
                });
                assert_eq!(
                    pending.actor.file_interpreter(false).await.unwrap(),
                    expected
                );
                assert_eq!(
                    pending.actor.file_interpreter(true).await.unwrap(),
                    expected
                );
            }
            assert!(events.try_recv().is_err());
        }
        for shell in [ShellKind::Serial, ShellKind::Telnet] {
            pending.actor.cfg.shell_kind = shell;
            assert!(pending.actor.file_interpreter(false).await.is_err());
            assert!(events.try_recv().is_err());
        }
    }

    fn probe_session() -> (PendingSession, ProbeEvents) {
        let (host, events) = crate::ssh::test_support::headless_host();
        let cfg = SessionConfig {
            tab_id: "caps-test".into(),
            target_id: "target".into(),
            skill: "general".into(),
            system_prompt: String::new(),
            response_language: "English".into(),
            user_skills_cache: Vec::new(),
            model: "test".into(),
            provider: "test".into(),
            // These tests drive the probe directly and never call the LLM.
            driver: crate::ai::session::SessionDriver::Ai(Box::new(
                crate::ai::llm::OpenAiCompletionsClient::new(
                    String::new(),
                    "http://127.0.0.1:1".into(),
                ),
            )),
            redact_rules: Vec::new(),
            blacklist: sanitize::Blacklist::default(),
            max_output_bytes: sanitize::DEFAULT_MAX_OUTPUT_BYTES,
            ssh_handle: None,
            data_dir: Default::default(),
            shell_kind: Default::default(),
            db: host.state().db.clone(),
            conversation_id: "conversation".into(),
            target_key: "local".into(),
            initial_history: Vec::new(),
            initial_audit: Default::default(),
        };
        (super::super::start(cfg, host).unwrap(), events)
    }

    async fn probe_with_result(
        pending: &mut PendingSession,
        events: &mut ProbeEvents,
        outcome: CommandOutcome,
    ) -> AppResult<RemoteCapabilities> {
        let action_tx = pending.info().action_tx.clone();
        let respond = async {
            let (event, payload) = events.recv().await.unwrap();
            assert_eq!(event, "ai:internal_command:caps-test");
            let command_id = payload["id"].as_str().unwrap().to_owned();
            let (ack, receipt) = oneshot::channel();
            let action = match outcome {
                CommandOutcome::Result {
                    exit_code,
                    output,
                    timed_out,
                    early_terminated,
                } => UserAction::CommandResult {
                    command_id,
                    exit_code,
                    output,
                    timed_out,
                    early_terminated,
                    ack: Some(ack),
                },
                CommandOutcome::Rejected { reason } => UserAction::RejectCommand {
                    command_id,
                    reason,
                    ack: Some(ack),
                },
            };
            action_tx.send(action).unwrap();
            // A failed probe still consumes its result. Rejecting this delivery
            // would leave the frontend retrying an already-consumed command.
            receipt.await.unwrap().unwrap();
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(pending.actor.ensure_remote_caps(), respond)
        })
        .await
        .expect("probe and processing acknowledgement must finish");
        result
    }

    #[tokio::test]
    async fn remote_caps_failed_probe_retries_without_caching_missing_tools() {
        let (mut pending, mut events) = probe_session();
        let refusal = "Command was not executed: the target terminal is in alternate screen mode";
        for (exit_code, timed_out, early_terminated, output) in [
            (-1, false, false, refusal),
            (1, false, false, "probe failed"),
            (0, true, false, "py3=1 perl=1 diff=1"),
            (0, false, true, "py3=1 perl=1 diff=1"),
            (0, false, false, "Invalid capability probe output"),
        ] {
            let error = probe_with_result(
                &mut pending,
                &mut events,
                CommandOutcome::Result {
                    exit_code,
                    output: output.into(),
                    timed_out,
                    early_terminated,
                },
            )
            .await
            .unwrap_err();
            assert_eq!(error.code(), "caps_probe_aborted");
            assert!(error.to_string().contains(output));
            assert_eq!(pending.actor.remote_caps, None);
        }

        let error = probe_with_result(
            &mut pending,
            &mut events,
            CommandOutcome::Rejected {
                reason: "user cancelled".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("user cancelled"));
        assert_eq!(pending.actor.remote_caps, None);

        for output in ["py3=0 perl=0 diff=0", "py3=1 perl=0 diff=0"] {
            probe_with_result(
                &mut pending,
                &mut events,
                CommandOutcome::Result {
                    exit_code: 0,
                    output: output.into(),
                    timed_out: false,
                    early_terminated: false,
                },
            )
            .await
            .unwrap();
            assert_eq!(
                pending.actor.remote_caps, None,
                "negative capabilities must be retried"
            );
        }

        let caps = probe_with_result(
            &mut pending,
            &mut events,
            CommandOutcome::Result {
                exit_code: 0,
                output: "py3=1 perl=0 diff=1".into(),
                timed_out: false,
                early_terminated: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            caps,
            RemoteCapabilities {
                python3: true,
                perl: false,
                diff: true
            }
        );
        assert_eq!(pending.actor.remote_caps, Some(caps));
        assert_eq!(pending.actor.ensure_remote_caps().await.unwrap(), caps);
        assert!(
            events.try_recv().is_err(),
            "cached success must not probe again"
        );
    }

    // ─── shell_quote ────────────────────────────────────────────────

    #[test]
    fn shell_quote_plain_string() {
        // 无元字符的字符串，直接套单引号即可
        assert_eq!(shell_quote("hello"), "'hello'");
        assert_eq!(shell_quote("/tmp/foo.yml"), "'/tmp/foo.yml'");
    }

    #[test]
    fn shell_quote_empty_string() {
        // 空串也合法 —— shell 看到 '' 等同于空参数
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn shell_quote_with_spaces() {
        // 路径含空格是真实场景（用户主目录 / 含空格的文件名）
        assert_eq!(shell_quote("/tmp/has space/foo"), "'/tmp/has space/foo'");
    }

    #[test]
    fn shell_quote_single_quote_escape() {
        // 核心规则：单引号本身用 '\'' 拼出（关闭引号→反斜杠转义字面引号→重开引号）
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote("'"), "''\\'''");
        assert_eq!(shell_quote("a'b'c"), "'a'\\''b'\\''c'");
    }

    #[test]
    fn shell_quote_double_quote_passthrough() {
        // 双引号在单引号内按字面，不转义
        assert_eq!(shell_quote(r#"say "hi""#), r#"'say "hi"'"#);
    }

    #[test]
    fn shell_quote_dollar_and_backtick_neutralized() {
        // $VAR / `cmd` 在单引号内不展开 —— 这是用单引号而非双引号的原因
        assert_eq!(shell_quote("$HOME"), "'$HOME'");
        assert_eq!(shell_quote("`whoami`"), "'`whoami`'");
        assert_eq!(shell_quote("$(rm -rf /)"), "'$(rm -rf /)'");
    }

    #[test]
    fn shell_quote_shell_metacharacters_inert() {
        // |, &, ;, <, >, * 等在单引号内全部按字面
        assert_eq!(shell_quote("a|b&c;d"), "'a|b&c;d'");
        assert_eq!(shell_quote("> /etc/passwd"), "'> /etc/passwd'");
        assert_eq!(shell_quote("*"), "'*'");
        assert_eq!(shell_quote("~/foo"), "'~/foo'");
    }

    #[test]
    fn shell_quote_backslash_passthrough() {
        // 反斜杠在单引号内不是转义字符 —— 按字面输出
        assert_eq!(shell_quote(r"a\b"), r"'a\b'");
        assert_eq!(shell_quote(r"\n"), r"'\n'");
    }

    #[test]
    fn shell_quote_newline_preserved() {
        // 内容含换行也安全 —— shell 在单引号内保留 \n
        assert_eq!(shell_quote("line1\nline2"), "'line1\nline2'");
    }

    #[test]
    fn shell_quote_multibyte_chars() {
        // 非 ASCII（中文 / emoji 等）原样保留
        assert_eq!(shell_quote("文件名.txt"), "'文件名.txt'");
        assert_eq!(shell_quote("emoji 🦀"), "'emoji 🦀'");
    }

    #[test]
    fn shell_quote_idempotent_when_no_single_quote() {
        // 不含单引号的字符串两次套引号会双层（不是 idempotent —— 这是正确的，
        // 因为 shell_quote 输出本身含单引号，再次套需要二次转义）。
        // 这个测试只是文档化"不要重复调用"的约束。
        let once = shell_quote("foo");
        let twice = shell_quote(&once);
        assert_ne!(once, twice);
        assert!(twice.contains("\\'"));
    }

    // ─── validate_path ──────────────────────────────────────────────

    #[test]
    fn validate_path_rejects_newline_and_cr() {
        // 防御 ZLE race：path 含真 \n / \r 会让 shell_quote 输出含真换行 byte，破坏单行命令前提。
        assert!(validate_path("foo\nbar").is_err());
        assert!(validate_path("foo\rbar").is_err());
        assert!(validate_path("a\r\nb").is_err());
    }

    #[test]
    fn validate_path_rejects_other_c0_and_del() {
        // terminal injection 防御：path 含 ESC/BEL 等 C0 / DEL 也拒
        assert!(validate_path("foo\x1bbar").is_err());
        assert!(validate_path("foo\x07bar").is_err());
        assert!(validate_path("foo\x00bar").is_err());
        assert!(validate_path("foo\x7fbar").is_err());
    }

    #[test]
    fn validate_path_rejects_empty() {
        assert!(validate_path("").is_err());
    }

    #[test]
    fn validate_path_passes_normal_paths() {
        // 常规路径：绝对路径、~ 路径、含空格、中文、glob 字符（shell_quote 处理），都允许
        assert!(validate_path("/etc/hosts").is_ok());
        assert!(validate_path("~/foo.yml").is_ok());
        assert!(validate_path("/tmp/has space/file").is_ok());
        assert!(validate_path("文件名.txt").is_ok());
        assert!(validate_path("a'b").is_ok());
        assert!(validate_path("*.log").is_ok());
    }

    // ─── prepare_remote_path（~ 路径展开） ──────────────────────────

    #[test]
    fn prepare_path_plain_passes_through_to_shell_quote() {
        assert_eq!(prepare_remote_path("/tmp/foo"), "'/tmp/foo'");
        assert_eq!(prepare_remote_path("foo"), "'foo'");
        assert_eq!(prepare_remote_path(""), "''");
    }

    #[test]
    fn prepare_path_bare_tilde_expands_to_home() {
        // 单独的 ~ → "$HOME"（不带末尾斜杠）
        assert_eq!(prepare_remote_path("~"), "\"$HOME\"");
    }

    #[test]
    fn prepare_path_tilde_slash_expands_then_quotes_rest() {
        // ~/foo → "$HOME"/'foo'，shell 拼起来等同于 $HOME/foo
        assert_eq!(prepare_remote_path("~/foo"), "\"$HOME\"/'foo'");
        assert_eq!(prepare_remote_path("~/a/b/c.yml"), "\"$HOME\"/'a/b/c.yml'");
    }

    #[test]
    fn prepare_path_tilde_with_special_chars_in_rest() {
        // rest 含空格 / 单引号 必须被 shell_quote 正确处理
        assert_eq!(
            prepare_remote_path("~/has space/file"),
            "\"$HOME\"/'has space/file'"
        );
        assert_eq!(
            prepare_remote_path("~/it's.txt"),
            r#""$HOME"/'it'\''s.txt'"#
        );
    }

    #[test]
    fn prepare_path_other_user_home_not_expanded() {
        // ~user/... 形态：我们不展开（罕见，悄悄改写不如让用户看到明确路径错）
        // 走 shell_quote 的字面处理，shell 看到 '~user/foo' 找不到文件
        assert_eq!(prepare_remote_path("~root/foo"), "'~root/foo'");
        assert_eq!(prepare_remote_path("~user"), "'~user'");
    }

    #[test]
    fn prepare_path_tilde_in_middle_not_expanded() {
        // 只识别开头的 `~/` 或单独 `~`，路径中间的 ~ 不动
        assert_eq!(prepare_remote_path("/foo/~/bar"), "'/foo/~/bar'");
        assert_eq!(prepare_remote_path("foo~bar"), "'foo~bar'");
    }

    // ─── parse_capabilities ─────────────────────────────────────────

    #[test]
    fn parse_caps_all_present() {
        let r = parse_capabilities("py3=1 perl=1 diff=1\n").unwrap();
        assert!(r.python3 && r.perl && r.diff);
    }

    #[test]
    fn parse_caps_only_python3() {
        let r = parse_capabilities("py3=1 perl=0 diff=0\n").unwrap();
        assert!(r.python3 && !r.perl && !r.diff);
    }

    #[test]
    fn parse_caps_only_perl() {
        // 只有 perl —— match_file 能跑（不需要 diff），patch_file 在 handle 层另外校验 caps.diff
        let r = parse_capabilities("py3=0 perl=1 diff=0\n").unwrap();
        assert!(!r.python3 && r.perl && !r.diff);
        assert_eq!(select_interpreter(r), Some(Interpreter::Perl));
    }

    #[test]
    fn parse_caps_none() {
        let r = parse_capabilities("py3=0 perl=0 diff=0\n").unwrap();
        assert!(!r.python3 && !r.perl && !r.diff);
        assert_eq!(select_interpreter(r), None);
    }

    #[test]
    fn parse_caps_tolerates_pty_noise() {
        let out = format!("{PROBE_CMD}\npy3=1 perl=1 diff=1\nuser@host:~$ \n");
        let r = parse_capabilities(&out).unwrap();
        assert!(r.python3 && r.perl && r.diff);
    }

    #[test]
    fn parse_caps_missing_field_defaults_false() {
        // 缺一个字段 —— 整行不匹配，全部 false
        assert_eq!(parse_capabilities("py3=1 perl=1"), None);
    }

    #[test]
    fn parse_caps_non_one_treated_as_false() {
        assert_eq!(parse_capabilities("py3=yes perl=2 diff=true"), None);
    }

    #[test]
    fn parse_caps_empty_input() {
        assert_eq!(parse_capabilities(""), None);
    }

    // ─── select_interpreter ─────────────────────────────────────────

    #[test]
    fn select_interp_python3_wins() {
        // python3 优先于 perl
        let caps = RemoteCapabilities {
            python3: true,
            perl: true,
            diff: true,
        };
        assert_eq!(select_interpreter(caps), Some(Interpreter::Python3));
    }

    #[test]
    fn select_interp_python3_alone() {
        let caps = RemoteCapabilities {
            python3: true,
            perl: false,
            diff: false,
        };
        assert_eq!(select_interpreter(caps), Some(Interpreter::Python3));
    }

    #[test]
    fn select_interp_perl_alone_ok_for_match() {
        // 没有 python3，perl 单独足够给 match_file。patch_file 在 handle 层另查 caps.diff。
        let caps = RemoteCapabilities {
            python3: false,
            perl: true,
            diff: false,
        };
        assert_eq!(select_interpreter(caps), Some(Interpreter::Perl));
    }

    #[test]
    fn select_interp_none_without_interp() {
        // 有 diff 但没 python3 / perl —— file_ops 整体不可用
        let caps = RemoteCapabilities {
            python3: false,
            perl: false,
            diff: true,
        };
        assert_eq!(select_interpreter(caps), None);
    }

    // ─── extract_json_payload ───────────────────────────────────────

    #[test]
    fn extract_json_basic() {
        let pty =
            "shell echo\n__RSSH_JSON__\n{\"count\":3,\"matches\":[]}\n__RSSH_JSON__\nsentinel:0\n";
        assert_eq!(
            extract_json_payload(pty),
            Some("{\"count\":3,\"matches\":[]}")
        );
    }

    #[test]
    fn extract_json_with_pty_noise() {
        let pty = "\x1b[?2004l\rprefix\n__RSSH_JSON__\n{\"a\":1}\n__RSSH_JSON__\r\nsentinel\n";
        assert_eq!(extract_json_payload(pty), Some("{\"a\":1}"));
    }

    #[test]
    fn extract_json_missing_returns_none() {
        assert_eq!(extract_json_payload("no markers here"), None);
        assert_eq!(
            extract_json_payload("__RSSH_JSON__\nonly one marker\n"),
            None
        );
    }

    #[test]
    fn extract_json_ignores_marker_literal_in_echoed_script() {
        // Regression: shell ECHO 会把脚本源码一并回显，脚本里 `M = "__RSSH_JSON__"`
        // 那行会作为 echo 的一部分进入 buffer。此时 buffer 含 3 个 marker：
        // 1 个 echo 字面量 + 2 个脚本真正输出。必须抽脚本输出的那对。
        let pty = "\
python3 -c 'import sys\n\
M = \"__RSSH_JSON__\"\n\
print(M); print(\"payload\"); print(M)'\n\
__RSSH_JSON__\n\
{\"count\":5,\"matches\":[{\"line\":84}]}\n\
__RSSH_JSON__\n\
__rssh_done_abc:0\n";
        assert_eq!(
            extract_json_payload(pty),
            Some("{\"count\":5,\"matches\":[{\"line\":84}]}")
        );
    }

    #[test]
    fn extract_json_ignores_marker_literal_inside_json_payload() {
        // Regression：之前用 rfind 子串。如果用户文件里写了 `__RSSH_JSON__` 注释，
        // match_file 把 context 字段塞进 JSON 输出 —— rfind 会扫到 JSON 内部的 marker 子串，
        // 在 JSON 中间切，破解析。按"独立成行"匹配杜绝这个：marker 子串嵌在引号 + 上下文里
        // 时永远不会独占一行。
        let pty = "\
__RSSH_JSON__\n\
{\"count\":1,\"matches\":[{\"line\":3,\"context\":\"# note: __RSSH_JSON__ is rssh internal\"}]}\n\
__RSSH_JSON__\n\
__rssh_done_xyz:0\n";
        assert_eq!(
            extract_json_payload(pty),
            Some("{\"count\":1,\"matches\":[{\"line\":3,\"context\":\"# note: __RSSH_JSON__ is rssh internal\"}]}")
        );
    }

    #[test]
    fn extract_json_takes_last_pair_when_buffer_contains_multiple_runs() {
        // 防御性：PTY buffer 累积多次工具调用的输出时（实际不会，每次 finish 都清，
        // 但万一上游误传整段 session log），取最后一对仍然给出最新的 JSON。
        let pty = "\
__RSSH_JSON__\n{\"call\":\"first\"}\n__RSSH_JSON__\n\
some interleaved noise\n\
__RSSH_JSON__\n{\"call\":\"second\"}\n__RSSH_JSON__\n";
        assert_eq!(extract_json_payload(pty), Some("{\"call\":\"second\"}"));
    }

    #[test]
    fn every_shell_transmits_shared_scripts_and_nonempty_encoded_arguments() {
        for shell in [ShellKind::Posix, ShellKind::Powershell, ShellKind::Cmd] {
            for interp in [Interpreter::Python3, Interpreter::Perl] {
                for command in [
                    build_match_cmd(shell, interp, "~/it's %PATH% [中].txt", "a\n'b`$()", 80, 80),
                    build_modify_cmd(shell, interp, "~/it's %PATH% [中].tmp", "a\n'b`$()", "", 1),
                ] {
                    assert!(!command.contains(['\n', '\r']));
                    assert!(!command.contains("Rssh-Read"));
                    assert!(!command.contains("IndexOf"));
                    assert!(!command.contains("%PATH%"));
                }
            }
        }
    }

    #[test]
    fn posix_copy_diff_replace_preserve_literal_paths_and_home_expansion() {
        assert_eq!(
            build_cp_cmd(ShellKind::Posix, "/has space/it's", "/tmp/file"),
            r"\cp -- '/has space/it'\''s' '/tmp/file'"
        );
        assert_eq!(
            build_diff_cmd(ShellKind::Posix, "~/foo", "~/foo.tmp"),
            "\\diff -u -- \"$HOME\"/'foo' \"$HOME\"/'foo.tmp'"
        );
        assert_eq!(
            build_mv_cmd(ShellKind::Posix, "~/foo.tmp", "~/foo"),
            "\\mv -- \"$HOME\"/'foo.tmp' \"$HOME\"/'foo'"
        );
    }
}
