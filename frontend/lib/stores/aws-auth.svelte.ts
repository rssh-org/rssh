import { invoke } from "@tauri-apps/api/core";

export interface AwsSigninStatus {
  authenticated: boolean;
  account_id?: string | null;
  arn?: string | null;
}

export interface AwsSigninChallenge {
  login_id: string;
  authorization_url: string;
  expires_in: number;
}

export interface AwsAuthCommit {
  source_id: string;
  login_id: string;
}

interface SigninState {
  // Only a successful source save replaces this persisted identity.
  status: AwsSigninStatus;
  challenge: AwsSigninChallenge | null;
  draft: { loginId: string; status: AwsSigninStatus } | null;
  starting: boolean;
  busy: boolean;
  error: unknown;
}

// Only public login state lives here. Credentials stay in backend memory until
// Save commits them to SecretStore; authorization codes are never persisted.
let _sources = $state<Record<string, SigninState>>({});
const generations = new Map<string, number>();
const signedOut: AwsSigninStatus = { authenticated: false };

export function status(sourceId: string): AwsSigninStatus { return _sources[sourceId]?.draft?.status ?? _sources[sourceId]?.status ?? signedOut; }
export function challenge(sourceId: string): AwsSigninChallenge | null { return _sources[sourceId]?.challenge ?? null; }
export function commitRequest(sourceId: string): AwsAuthCommit | undefined {
  const draft = _sources[sourceId]?.draft;
  return draft ? { source_id: sourceId, login_id: draft.loginId } : undefined;
}
export function markCommitted(sourceId: string, loginId: string): void {
  const draft = _sources[sourceId]?.draft;
  if (draft?.loginId === loginId) update(sourceId, { status: draft.status, draft: null, error: null });
}
export function authorizing(sourceId: string): boolean { return !!_sources[sourceId]?.starting || challenge(sourceId) !== null || commitRequest(sourceId) !== undefined; }
export function busy(sourceId: string): boolean { return _sources[sourceId]?.busy ?? false; }
export function error(sourceId: string): unknown { return _sources[sourceId]?.error; }
export function setError(sourceId: string, error: unknown): void { update(sourceId, { error }); }

function update(sourceId: string, patch: Partial<SigninState>) {
  _sources[sourceId] = { status: signedOut, challenge: null, draft: null, starting: false, busy: false, error: null, ..._sources[sourceId], ...patch };
}

function advance(sourceId: string): number {
  const generation = (generations.get(sourceId) ?? 0) + 1;
  generations.set(sourceId, generation);
  return generation;
}

function current(sourceId: string, generation: number): boolean {
  return generations.get(sourceId) === generation;
}

export async function load(sourceId: string): Promise<void> {
  if (busy(sourceId) || challenge(sourceId) || commitRequest(sourceId)) return;
  const generation = advance(sourceId);
  update(sourceId, { busy: true, error: null });
  try {
    const next = await invoke<AwsSigninStatus>("aws_signin_status", { sourceId });
    if (current(sourceId, generation)) update(sourceId, { status: next });
  } catch (error) {
    if (current(sourceId, generation)) update(sourceId, { error });
  } finally {
    if (current(sourceId, generation)) update(sourceId, { busy: false });
  }
}

export async function start(sourceId: string, region: string): Promise<AwsSigninChallenge | null> {
  if (busy(sourceId) || challenge(sourceId) || commitRequest(sourceId)) return null;
  const generation = advance(sourceId);
  update(sourceId, { starting: true, busy: true, error: null });
  try {
    const next = await invoke<AwsSigninChallenge>("aws_signin_start", { sourceId, region });
    if (!current(sourceId, generation)) {
      // The form can close while the request is in flight. Cancel the returned
      // backend attempt instead of resurrecting it in another form.
      await invoke("aws_signin_cancel", { sourceId, loginId: next.login_id });
      return null;
    }
    update(sourceId, { challenge: next });
    return next;
  } catch (error) {
    if (current(sourceId, generation)) update(sourceId, { error });
    else console.warn("[aws-signin] cancelled attempt cleanup failed:", error);
    return null;
  } finally {
    if (current(sourceId, generation)) update(sourceId, { starting: false, busy: false });
  }
}

export async function complete(sourceId: string, code: string): Promise<boolean> {
  const login = challenge(sourceId);
  if (busy(sourceId) || !login || !code.trim()) return false;
  const generation = advance(sourceId);
  update(sourceId, { busy: true, error: null });
  try {
    const next = await invoke<AwsSigninStatus>("aws_signin_complete", { sourceId, loginId: login.login_id, code: code.trim() });
    if (!current(sourceId, generation)) return false;
    if (next.authenticated) update(sourceId, { draft: { loginId: login.login_id, status: next }, challenge: null });
    return next.authenticated;
  } catch (error) {
    if (current(sourceId, generation)) update(sourceId, { error });
    return false;
  } finally {
    if (current(sourceId, generation)) update(sourceId, { busy: false });
  }
}

export async function cancel(sourceId: string): Promise<void> {
  const loginId = challenge(sourceId)?.login_id ?? commitRequest(sourceId)?.login_id;
  const generation = advance(sourceId);
  update(sourceId, { challenge: null, draft: null, starting: false, busy: false, error: null });
  if (!loginId) return;
  try {
    await invoke("aws_signin_cancel", { sourceId, loginId });
  } catch (error) {
    if (current(sourceId, generation)) update(sourceId, { error });
    throw error;
  }
}

export async function logout(sourceId: string): Promise<void> {
  await cancel(sourceId);
  const generation = advance(sourceId);
  update(sourceId, { busy: true, error: null });
  try {
    await invoke("aws_signin_logout", { sourceId });
    if (current(sourceId, generation)) update(sourceId, { status: signedOut });
  } catch (error) {
    if (current(sourceId, generation)) update(sourceId, { error });
    throw error;
  } finally {
    if (current(sourceId, generation)) update(sourceId, { busy: false });
  }
}
