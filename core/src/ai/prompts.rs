//! 内置 skill 的 prompt 内容 —— 编译时 `include_str!` 内嵌进二进制。
//! `general` 直接进入 system prompt；其余内置 skill 只在目录中暴露，按需加载。

pub const GENERAL: &str = include_str!("prompts/general.md");
