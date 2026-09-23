import fs from '@ohos.file.fs';
import fileUri from '@ohos.file.fileuri';
import { BusinessError } from '@kit.BasicServicesKit';
import { AsyncPluginBase, BridgeCallContext, BridgeContextRequirement, BridgeTypedValue } from '@ohos-rs/ability';

interface OpenRequest {
  uri: string;
  write: boolean;
  createInDirectory: boolean;
}

interface NameRequest {
  uri: string;
}

class FileName {
  readonly name: string;
  constructor(name: string) { this.name = name; }
}

class DownloadPath {
  readonly path: string;
  constructor(path: string) { this.path = path; }
}

interface DirectoryRequest {
  uri: string;
  write: boolean;
  relativePaths: string[];
}

class BorrowedFile {
  readonly fd: number;
  constructor(fd: number) { this.fd = fd; }
}

class FileTreeEntry {
  readonly uri: string;
  readonly relativePath: string;
  readonly size: number;
  constructor(uri: string, relativePath: string, size: number) {
    this.uri = uri;
    this.relativePath = relativePath;
    this.size = size;
  }
}

class FileTree {
  readonly entries: FileTreeEntry[];
  constructor(entries: FileTreeEntry[]) { this.entries = entries; }
}

class ResolvedFile {
  readonly relativePath: string;
  readonly location?: string;
  readonly error?: string;
  constructor(relativePath: string, location?: string, error?: string) {
    this.relativePath = relativePath;
    this.location = location;
    this.error = error;
  }
}

class ResolvedPaths {
  readonly files: ResolvedFile[];
  constructor(files: ResolvedFile[]) { this.files = files; }
}

class PendingDirectory {
  readonly relativePath: string;
  readonly depth: number;
  constructor(relativePath: string, depth: number) {
    this.relativePath = relativePath;
    this.depth = depth;
  }
}

function ensureActive(context: BridgeCallContext): void {
  if (!context.isActive() || context.isCancelled()) throw new Error('File access was cancelled');
}

function pathSegments(relativePath: string): string[] {
  const segments = relativePath.split('/');
  for (const segment of segments) {
    if (!segment || segment === '.' || segment === '..' || /[\\\x00-\x1f\x7f]/.test(segment)) {
      throw new Error('Expected a relative file name below the selected directory');
    }
  }
  return segments;
}

function childUri(directory: string, relativePath: string): string {
  return directory.replace(/\/+$/, '') + '/' + pathSegments(relativePath).map((part: string) => encodeURIComponent(part)).join('/');
}

async function requireDirectory(path: string): Promise<void> {
  const stat = await fs.lstat(path);
  if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('Selected file tree contains a non-directory or symbolic link');
}

/** FileUri.path is the platform conversion for APIs accepting sandbox paths.
 * Conversion stays here; callers continue to own the selected URI. */
async function prepareTarget(directory: string, relativePath: string, context: BridgeCallContext): Promise<string> {
  const segments = pathSegments(relativePath);
  let path = new fileUri.FileUri(directory).path;
  await requireDirectory(path);
  for (let index = 0; index < segments.length - 1; index++) {
    ensureActive(context);
    path += '/' + segments[index];
    try {
      await fs.mkdir(path);
    } catch (error) {
      const cause = error as BusinessError;
      if (cause.code !== 13900015) throw error as Error; // EEXIST
    }
    await requireDirectory(path);
  }
  const target = childUri(directory, relativePath);
  const targetPath = new fileUri.FileUri(target).path;
  try {
    const stat = await fs.lstat(targetPath);
    if (!stat.isFile() || stat.isSymbolicLink()) throw new Error('Download target must be a regular file');
  } catch (error) {
    const cause = error as BusinessError;
    if (cause.code !== 13900002) throw error as Error; // ENOENT is a new file.
  }
  ensureActive(context);
  return target;
}

async function resolveSource(directory: string, relativePath: string, context: BridgeCallContext): Promise<string> {
  const segments = pathSegments(relativePath);
  let path = new fileUri.FileUri(directory).path;
  await requireDirectory(path);
  for (let index = 0; index < segments.length; index++) {
    ensureActive(context);
    path += '/' + segments[index];
    const stat = await fs.lstat(path);
    if (stat.isSymbolicLink()) throw new Error('Selected file tree contains a symbolic link');
    const expectedType = index === segments.length - 1 ? stat.isFile() : stat.isDirectory();
    if (!expectedType) throw new Error('Source must be a regular file below the selected directory');
  }
  ensureActive(context);
  return childUri(directory, relativePath);
}

async function walkDirectory(directory: string, context: BridgeCallContext): Promise<FileTree> {
  const rootPath = new fileUri.FileUri(directory).path;
  await requireDirectory(rootPath);
  const queue: PendingDirectory[] = [new PendingDirectory('', 0)];
  const entries: FileTreeEntry[] = [];
  for (let index = 0; index < queue.length; index++) {
    ensureActive(context);
    const current = queue[index];
    if (current.depth >= 32) throw new Error('Local file tree exceeds the maximum depth of 32');
    const path = current.relativePath ? rootPath + '/' + current.relativePath : rootPath;
    await requireDirectory(path);
    const names = await fs.listFile(path);
    for (const name of names) {
      ensureActive(context);
      pathSegments(name);
      const relativePath = current.relativePath ? current.relativePath + '/' + name : name;
      const stat = await fs.lstat(path + '/' + name);
      if (stat.isSymbolicLink()) continue;
      if (stat.isDirectory()) queue.push(new PendingDirectory(relativePath, current.depth + 1));
      if (stat.isFile()) entries.push(new FileTreeEntry(childUri(directory, relativePath), relativePath, stat.size));
    }
  }
  ensureActive(context);
  return new FileTree(entries);
}

/** Rust validates exact picker grants; this adapter converts URI semantics and
 * owns each platform File only while transferring its duplicated descriptor. */
export class FilesAccessPlugin extends AsyncPluginBase {
  readonly id = 'rssh.files-access';
  readonly requires: BridgeContextRequirement[] = ['ability'];

  async invokeAsync(action: string, payload: BridgeTypedValue, context: BridgeCallContext): Promise<BridgeTypedValue> {
    ensureActive(context);
    if (!payload.value || typeof payload.value !== 'object') throw new Error('File access requires a request');
    if (action === 'file-name') {
      if (payload.typeName !== 'rssh.files.NameRequest') throw new Error('file-name requires rssh.files.NameRequest');
      const request = payload.value as NameRequest;
      if (typeof request.uri !== 'string' || !request.uri) throw new Error('File name requires a URI');
      return { typeName: 'rssh.files.FileName', value: new FileName(new fileUri.FileUri(request.uri).name) };
    }
    if (action === 'download-path') {
      if (payload.typeName !== 'rssh.files.NameRequest') throw new Error('download-path requires rssh.files.NameRequest');
      const request = payload.value as NameRequest;
      if (typeof request.uri !== 'string' || !request.uri) throw new Error('Download path requires a URI');
      return { typeName: 'rssh.files.DownloadPath', value: new DownloadPath(new fileUri.FileUri(request.uri).path) };
    }
    if (action === 'open-file') {
      if (payload.typeName !== 'rssh.files.OpenRequest') throw new Error('open-file requires rssh.files.OpenRequest');
      const request = payload.value as OpenRequest;
      if (typeof request.uri !== 'string' || !request.uri || typeof request.write !== 'boolean' ||
        typeof request.createInDirectory !== 'boolean' || (request.createInDirectory && !request.write)) {
        throw new Error('open-file requires a URI and a boolean write mode');
      }
      const mode = request.write ? fs.OpenMode.WRITE_ONLY | fs.OpenMode.CREATE | fs.OpenMode.TRUNC : fs.OpenMode.READ_ONLY;
      // A nonexistent docs URI is sent to DataShare by fs.open, which cannot
      // create a child. Only Rust's exact directory-child grant enables the
      // sandbox path route; picker-selected files keep their URI provider.
      const location = request.createInDirectory ? new fileUri.FileUri(request.uri).path : request.uri;
      const file = fs.openSync(location, mode);
      try {
        const owned: ESObject = context.invokeNativeSync(
          'duplicate-file', 'rssh.files.BorrowedFile', 'rssh.files.OpenedFile', new BorrowedFile(file.fd),
        );
        return { typeName: 'rssh.files.OpenedFile', value: owned };
      } finally {
        fs.closeSync(file);
      }
    }
    if (payload.typeName !== 'rssh.files.DirectoryRequest') throw new Error('Directory access requires rssh.files.DirectoryRequest');
    const request = payload.value as DirectoryRequest;
    if (typeof request.uri !== 'string' || !request.uri || typeof request.write !== 'boolean' || !Array.isArray(request.relativePaths)) {
      throw new Error('Directory access requires a URI, write mode and relative names');
    }
    if (action === 'walk-directory') {
      return { typeName: 'rssh.files.FileTree', value: await walkDirectory(request.uri, context) };
    }
    if (action === 'resolve-paths') {
      const files: ResolvedFile[] = [];
      for (const relativePath of request.relativePaths) {
        ensureActive(context);
        try {
          const location = request.write
            ? await prepareTarget(request.uri, relativePath, context)
            : await resolveSource(request.uri, relativePath, context);
          files.push(new ResolvedFile(relativePath, location));
        } catch (error) {
          ensureActive(context);
          const cause = error as BusinessError;
          files.push(new ResolvedFile(relativePath, undefined, cause.message));
        }
      }
      return { typeName: 'rssh.files.ResolvedPaths', value: new ResolvedPaths(files) };
    }
    throw new Error(`Unsupported file access action '${action}'`);
  }
}
