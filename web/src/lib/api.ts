import { auth, type Session } from './auth.svelte';

export const API_BASE =
	(import.meta.env.VITE_API_BASE as string | undefined) ?? 'http://localhost:3080';

export type { Session };
export type Conversation = {
	id: string;
	user_id: string;
	agent_id?: string | null;
	title: string;
	pinned: boolean;
	archived: boolean;
	created_at: string;
	updated_at: string;
};

export type Message = {
	id: string;
	conversation_id: string;
	role: string;
	content?: string | null;
	reasoning?: string | null;
	tool_calls?: unknown;
	tool_call_id?: string | null;
	status?: string;
	created_at: string;
};

export type Agent = {
	id: string;
	user_id: string;
	name: string;
	instructions?: string | null;
	provider_id?: string | null;
	model?: string | null;
	tools: string[];
	sandbox_enabled: boolean;
	created_at: string;
	updated_at: string;
};

export type CreateAgentInput = {
	name: string;
	instructions?: string;
	provider_id?: string;
	model?: string;
	tools?: string[];
	sandbox_enabled?: boolean;
};

export type Provider = {
	id: string;
	user_id?: string | null;
	name: string;
	kind: string;
	base_url: string;
	/** Whether the provider serves models that accept image input. */
	vision: boolean;
	created_at: string;
	/** Whether the caller has a usable key (own credential or shared key). */
	has_key?: boolean;
};

export type CreateProviderInput = {
	name: string;
	base_url: string;
	kind?: string;
	api_key?: string;
	vision?: boolean;
	global?: boolean;
};

export type ModelInfo = { id: string; owned_by?: string | null };

/** An OpenAI-style tool call emitted by the agent loop. */
export type ToolCall = {
	id: string;
	type?: string;
	function: { name: string; arguments: string };
};

/** The result of running a tool call, keyed by the originating call id. */
export type ToolResult = {
	tool_call_id: string;
	name: string;
	content: string;
};

export type RejectedFile = {
	name: string;
	reason: string;
};

export type ExecResult = {
	exit_code: number;
	stdout: string;
	stderr: string;
	timed_out: boolean;
	truncated?: boolean;
	files?: { name: string; content_b64: string }[];
	rejected_files?: RejectedFile[];
};

export type Computer = {
	id: string;
	user_id: string;
	node: string;
	handle?: string | null;
	volume?: string | null;
	state: string;
	created_at: string;
	updated_at: string;
	last_active_at: string;
};

export type ComputerExecInput = {
	language: string;
	code: string;
	files?: { name: string; content_b64: string }[];
	conversation_id?: string;
};

export type SandboxRequest = {
	language: string;
	code: string;
	files?: { name: string; content_b64: string }[];
};

export type FileRecord = {
	id: string;
	user_id: string;
	conversation_id?: string | null;
	filename: string;
	mime?: string | null;
	size_bytes: number;
	storage_path: string;
	created_at: string;
};

export type UploadInput = {
	filename: string;
	content_b64: string;
	mime?: string;
	conversation_id?: string;
};

export type UploadResponse = {
	file: FileRecord;
};

/** An account as returned by the admin user-management endpoints. */
export type Account = {
	id: string;
	email: string;
	name?: string | null;
	role: string;
	disabled: boolean;
	locked_until?: string | null;
	created_at: string;
	last_seen_at?: string | null;
};

/** A row of `GET /api/admin/users`: profile plus owned-resource counts. */
export type AdminUserSummary = Account & {
	conversation_count: number;
	provider_count: number;
	agent_count: number;
};

export type UpdateAdminUserInput = { role?: string; disabled?: boolean; unlock?: boolean };

export type CreateAdminUserInput = {
	email: string;
	password: string;
	name?: string;
	role?: string;
};

export type UpdateProviderInput = {
	name?: string;
	base_url?: string;
	kind?: string;
	api_key?: string;
	vision?: boolean;
};

/** One entry of the security audit trail (`GET /api/admin/audit`). */
export type AuditLog = {
	id: string;
	actor_id?: string | null;
	action: string;
	target_type?: string | null;
	target_id?: string | null;
	metadata?: Record<string, unknown> | null;
	ip?: string | null;
	created_at: string;
};

export class ApiError extends Error {
	status: number;
	constructor(status: number, message: string) {
		super(message);
		this.name = 'ApiError';
		this.status = status;
	}
}

function doFetch(path: string, init: RequestInit): Promise<Response> {
	const headers = new Headers(init.headers);
	if (init.body && !headers.has('content-type')) headers.set('content-type', 'application/json');
	const token = auth.token;
	if (token) headers.set('authorization', `Bearer ${token}`);
	return fetch(`${API_BASE}${path}`, { ...init, headers });
}

let refreshInFlight: Promise<boolean> | null = null;

/**
 * Exchange the stored refresh token for a fresh access + refresh pair. On
 * success the new session replaces the stored one; returns whether it worked.
 */
export async function refreshSession(): Promise<boolean> {
	const refresh_token = auth.refreshToken;
	if (!refresh_token) return false;
	try {
		const res = await fetch(`${API_BASE}/api/auth/refresh`, {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ refresh_token })
		});
		if (!res.ok) return false;
		auth.set((await res.json()) as Session);
		return true;
	} catch {
		return false;
	}
}

/** De-duplicate concurrent refreshes so a burst of 401s rotates only once. */
function refreshOnce(): Promise<boolean> {
	refreshInFlight ??= refreshSession().finally(() => {
		refreshInFlight = null;
	});
	return refreshInFlight;
}

async function request<T>(path: string, init: RequestInit = {}, retry = true): Promise<T> {
	const res = await doFetch(path, init);
	// A short-lived access token may have expired: refresh once and replay.
	if (res.status === 401 && retry && auth.refreshToken) {
		if (await refreshOnce()) return request<T>(path, init, false);
	}
	if (!res.ok) throw await toApiError(res);
	if (res.status === 204) return undefined as T;
	return (await res.json()) as T;
}

/** Revoke the current refresh token server-side (best-effort) then return. */
export async function logout(refreshToken: string | null = auth.refreshToken): Promise<void> {
	try {
		await fetch(`${API_BASE}/api/auth/logout`, {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ refresh_token: refreshToken })
		});
	} catch {
		// Best-effort: the client clears its session regardless.
	}
}

async function toApiError(res: Response): Promise<ApiError> {
	let message = res.statusText || `request failed (${res.status})`;
	try {
		const data = (await res.json()) as { error?: string };
		if (data?.error) message = data.error;
	} catch {
		// non-JSON error body; keep the status text
	}
	return new ApiError(res.status, message);
}

export function register(email: string, password: string, name?: string): Promise<Session> {
	return request<Session>('/api/auth/register', {
		method: 'POST',
		body: JSON.stringify({ email, password, name })
	});
}

export function login(email: string, password: string): Promise<Session> {
	return request<Session>('/api/auth/login', {
		method: 'POST',
		body: JSON.stringify({ email, password })
	});
}

/** Confirm an address with the token from a verification link. */
export function verifyEmail(token: string): Promise<void> {
	return request<void>('/api/auth/verify-email', {
		method: 'POST',
		body: JSON.stringify({ token })
	});
}

/** Send a fresh verification link; always succeeds, to avoid enumeration. */
export function resendVerification(email: string): Promise<void> {
	return request<void>('/api/auth/resend-verification', {
		method: 'POST',
		body: JSON.stringify({ email })
	});
}

/** Send a password reset link; always succeeds, to avoid enumeration. */
export function requestPasswordReset(email: string): Promise<void> {
	return request<void>('/api/auth/password-reset', {
		method: 'POST',
		body: JSON.stringify({ email })
	});
}

/** Set a new password with the token from a reset link. */
export function confirmPasswordReset(token: string, password: string): Promise<void> {
	return request<void>('/api/auth/password-reset/confirm', {
		method: 'POST',
		body: JSON.stringify({ token, password })
	});
}

export function listConversations(archived = false): Promise<Conversation[]> {
	return request<Conversation[]>(`/api/conversations${archived ? '?archived=true' : ''}`);
}

export function createConversation(title: string, agentId?: string): Promise<Conversation> {
	return request<Conversation>('/api/conversations', {
		method: 'POST',
		body: JSON.stringify({ title, agent_id: agentId ?? null })
	});
}

export function deleteConversation(conversationId: string): Promise<void> {
	return request<void>(`/api/conversations/${conversationId}`, { method: 'DELETE' });
}

export function setConversationAgent(
	conversationId: string,
	agentId: string | null
): Promise<Conversation> {
	return request<Conversation>(`/api/conversations/${conversationId}`, {
		method: 'PATCH',
		body: JSON.stringify({ agent_id: agentId })
	});
}

export function renameConversation(conversationId: string, title: string): Promise<Conversation> {
	return request<Conversation>(`/api/conversations/${conversationId}`, {
		method: 'PATCH',
		body: JSON.stringify({ title })
	});
}

export function setConversationArchived(
	conversationId: string,
	archived: boolean
): Promise<Conversation> {
	return request<Conversation>(`/api/conversations/${conversationId}`, {
		method: 'PATCH',
		body: JSON.stringify({ archived })
	});
}

export function setConversationPinned(
	conversationId: string,
	pinned: boolean
): Promise<Conversation> {
	return request<Conversation>(`/api/conversations/${conversationId}`, {
		method: 'PATCH',
		body: JSON.stringify({ pinned })
	});
}

export function duplicateConversation(conversationId: string): Promise<Conversation> {
	return request<Conversation>(`/api/conversations/${conversationId}/duplicate`, {
		method: 'POST'
	});
}

export function listAgents(): Promise<Agent[]> {
	return request<Agent[]>('/api/agents');
}

export function createAgent(input: CreateAgentInput): Promise<Agent> {
	return request<Agent>('/api/agents', {
		method: 'POST',
		body: JSON.stringify(input)
	});
}

export function deleteAgent(agentId: string): Promise<void> {
	return request<void>(`/api/agents/${agentId}`, { method: 'DELETE' });
}

export function listProviders(): Promise<Provider[]> {
	return request<Provider[]>('/api/providers');
}

export function createProvider(input: CreateProviderInput): Promise<Provider> {
	return request<Provider>('/api/providers', {
		method: 'POST',
		body: JSON.stringify(input)
	});
}

export function deleteProvider(providerId: string): Promise<void> {
	return request<void>(`/api/providers/${providerId}`, { method: 'DELETE' });
}

export function updateProvider(
	providerId: string,
	input: UpdateProviderInput
): Promise<Provider> {
	return request<Provider>(`/api/providers/${providerId}`, {
		method: 'PATCH',
		body: JSON.stringify(input)
	});
}

/**
 * Store the caller's own API key for a provider (used for admin-provisioned
 * global providers that ship without a key). Send an empty string to clear it.
 */
export function setProviderCredential(providerId: string, apiKey: string): Promise<Provider> {
	return request<Provider>(`/api/providers/${providerId}/credential`, {
		method: 'PUT',
		body: JSON.stringify({ api_key: apiKey })
	});
}

/** The cached model catalog for one provider (own or global). */
export function listProviderModels(providerId: string): Promise<ModelInfo[]> {
	return request<ModelInfo[]>(`/api/providers/${providerId}/models`);
}

// ---- admin ----------------------------------------------------------------

export function listAdminUsers(): Promise<AdminUserSummary[]> {
	return request<AdminUserSummary[]>('/api/admin/users');
}

export function createAdminUser(input: CreateAdminUserInput): Promise<Account> {
	return request<Account>('/api/admin/users', {
		method: 'POST',
		body: JSON.stringify(input)
	});
}

export function updateAdminUser(id: string, input: UpdateAdminUserInput): Promise<Account> {
	return request<Account>(`/api/admin/users/${id}`, {
		method: 'PATCH',
		body: JSON.stringify(input)
	});
}

export function deleteAdminUser(id: string): Promise<void> {
	return request<void>(`/api/admin/users/${id}`, { method: 'DELETE' });
}

export function listAudit(limit = 100): Promise<AuditLog[]> {
	return request<AuditLog[]>(`/api/admin/audit?limit=${limit}`);
}

/** Create several instance-wide (global) providers in one request. */
export function bulkCreateProviders(providers: CreateProviderInput[]): Promise<Provider[]> {
	return request<Provider[]>('/api/admin/providers', {
		method: 'POST',
		body: JSON.stringify({ providers })
	});
}

export function listMessages(
	conversationId: string,
	opts: { limit?: number; before?: string; after?: string } = {}
): Promise<Message[]> {
	const params = new URLSearchParams();
	if (opts.limit !== undefined) params.set('limit', String(opts.limit));
	if (opts.before) params.set('before', opts.before);
	if (opts.after) params.set('after', opts.after);
	const query = params.toString();
	return request<Message[]>(
		`/api/conversations/${conversationId}/messages${query ? `?${query}` : ''}`
	);
}

export function runSandbox(input: SandboxRequest): Promise<ExecResult> {
	return request<ExecResult>('/api/sandbox/run', {
		method: 'POST',
		body: JSON.stringify(input)
	});
}

export function getComputer(): Promise<Computer> {
	return request<Computer>('/api/computers/me');
}

export function pauseComputer(): Promise<Computer | null> {
	return request<Computer | null>('/api/computers/me/pause', { method: 'POST' });
}

export function resumeComputer(): Promise<Computer | null> {
	return request<Computer | null>('/api/computers/me/resume', { method: 'POST' });
}

export function destroyComputer(): Promise<{ destroyed: boolean }> {
	return request<{ destroyed: boolean }>('/api/computers/me', { method: 'DELETE' });
}

export function execComputer(input: ComputerExecInput): Promise<ExecResult> {
	return request<ExecResult>('/api/computers/exec', {
		method: 'POST',
		body: JSON.stringify(input)
	});
}

export function uploadFileWithProgress(
	input: UploadInput,
	onProgress: (fraction: number) => void,
	signal?: AbortSignal
): Promise<UploadResponse> {
	const send = (): Promise<UploadResponse> =>
		new Promise((resolve, reject) => {
			const xhr = new XMLHttpRequest();
			xhr.open('POST', `${API_BASE}/api/files`);
			xhr.setRequestHeader('content-type', 'application/json');
			const token = auth.token;
			if (token) xhr.setRequestHeader('authorization', `Bearer ${token}`);
			xhr.upload.onprogress = (event) => {
				if (event.lengthComputable && event.total > 0) onProgress(event.loaded / event.total);
			};
			xhr.onload = () => {
				if (xhr.status >= 200 && xhr.status < 300) {
					try {
						resolve(JSON.parse(xhr.responseText) as UploadResponse);
					} catch (err) {
						reject(err);
					}
					return;
				}
				let message = xhr.statusText || `request failed (${xhr.status})`;
				try {
					const data = JSON.parse(xhr.responseText) as { error?: string };
					if (data?.error) message = data.error;
				} catch {
				}
				reject(new ApiError(xhr.status, message));
			};
			xhr.onerror = () => reject(new Error('network error during upload'));
			xhr.onabort = () => reject(new DOMException('Aborted', 'AbortError'));
			if (signal) signal.addEventListener('abort', () => xhr.abort(), { once: true });
			xhr.send(JSON.stringify(input));
		});
	return send().catch(async (err) => {
		if (
			err instanceof ApiError &&
			err.status === 401 &&
			auth.refreshToken &&
			(await refreshOnce())
		) {
			return send();
		}
		throw err;
	});
}

export function listFiles(conversationId: string): Promise<FileRecord[]> {
	return request<FileRecord[]>(`/api/conversations/${conversationId}/files`);
}

/** The caller's personal file library (`GET /api/files`). */
export function listUserFiles(): Promise<FileRecord[]> {
	return request<FileRecord[]>('/api/files');
}

/** Attach existing library files to a conversation; returns the new list. */
export function attachConversationFiles(
	conversationId: string,
	fileIds: string[]
): Promise<FileRecord[]> {
	return request<FileRecord[]>(`/api/conversations/${conversationId}/files`, {
		method: 'POST',
		body: JSON.stringify({ file_ids: fileIds })
	});
}

/** Detach a file from a conversation, keeping it in the library. */
export function detachConversationFile(conversationId: string, fileId: string): Promise<void> {
	return request<void>(`/api/conversations/${conversationId}/files/${fileId}`, {
		method: 'DELETE'
	});
}

/** Remove a file from the library entirely. */
export function deleteFile(fileId: string): Promise<void> {
	return request<void>(`/api/files/${fileId}`, { method: 'DELETE' });
}

/**
 * Download a library file to the user's machine. The endpoint is bearer-auth'd,
 * so we fetch the blob and trigger a save instead of using a plain link.
 */
export async function downloadFile(file: FileRecord): Promise<void> {
	const open = () => {
		const headers = new Headers();
		const token = auth.token;
		if (token) headers.set('authorization', `Bearer ${token}`);
		return fetch(`${API_BASE}/api/files/${file.id}`, { headers });
	};
	let res = await open();
	if (res.status === 401 && auth.refreshToken && (await refreshOnce())) res = await open();
	if (!res.ok) throw await toApiError(res);
	const blob = await res.blob();
	const url = URL.createObjectURL(blob);
	const link = document.createElement('a');
	link.href = url;
	link.download = file.filename;
	document.body.appendChild(link);
	link.click();
	link.remove();
	URL.revokeObjectURL(url);
}

export type StreamHandlers = {
	onMeta?: (meta: StreamMeta) => void;
	onDelta?: (text: string) => void;
	onThinking?: (text: string) => void;
	onToolCall?: (calls: ToolCall[]) => void;
	onToolResult?: (result: ToolResult) => void;
	onReconnecting?: (attempt: number) => void;
	onReconnected?: () => void;
	onError?: (error: unknown) => void;
	onDone?: () => void;
};

export type StreamMeta = {
	message_id: string;
	conversation_id: string;
	resume_path?: string;
	resumed?: boolean;
};

export type ChatRequest = {
	conversation_id: string;
	content: string;
	model?: string;
	provider_id?: string;
};

const RECONNECT_ATTEMPTS = 8;
const RECONNECT_BASE_DELAY_MS = 700;

/**
 *
 * `EventSource` cannot issue a POST, so we read the `text/event-stream` body
 */
export async function streamChat(
	input: ChatRequest,
	handlers: StreamHandlers,
	signal?: AbortSignal
): Promise<void> {
	const open = (): Promise<Response> => {
		const headers = new Headers({ 'content-type': 'application/json' });
		const token = auth.token;
		if (token) headers.set('authorization', `Bearer ${token}`);
		return fetch(`${API_BASE}/api/chat`, {
			method: 'POST',
			headers,
			body: JSON.stringify(input),
			signal
		});
	};

	const resume = (path: string, lastEventId: number): Promise<Response> => {
		const headers = new Headers({ accept: 'text/event-stream' });
		if (lastEventId > 0) headers.set('last-event-id', String(lastEventId));
		const token = auth.token;
		if (token) headers.set('authorization', `Bearer ${token}`);
		return fetch(`${API_BASE}${path}`, { headers, signal });
	};

	let res: Response;
	try {
		res = await open();
		// A stale access token yields 401 before the stream starts: refresh once.
		if (res.status === 401 && auth.refreshToken && (await refreshOnce())) {
			res = await open();
		}
	} catch (err) {
		if (!isAbort(err)) handlers.onError?.(err);
		return;
	}

	if (!res.ok) {
		handlers.onError?.(await toApiError(res));
		return;
	}
	if (!res.body) {
		handlers.onError?.(new Error('streaming not supported by this browser'));
		return;
	}

	let lastEventId = 0;
	let resumePath: string | null = null;

	const consume = async (response: Response): Promise<'done' | 'dropped' | 'aborted'> => {
		if (!response.body) return 'dropped';
		const reader = response.body.getReader();
		const decoder = new TextDecoder();
		let buffer = '';
		try {
			for (;;) {
				const { value, done } = await reader.read();
				if (done) break;
				buffer += decoder.decode(value, { stream: true });

				let boundary: number;
				while ((boundary = buffer.search(/\r?\n\r?\n/)) !== -1) {
					const match = buffer.slice(boundary).match(/^\r?\n\r?\n/);
					const frame = buffer.slice(0, boundary);
					buffer = buffer.slice(boundary + (match ? match[0].length : 0));

					const event = parseFrame(frame);
					if (!event) continue;
					if (event.id !== null) lastEventId = event.id;
					if (event.event === 'meta') {
						try {
							const meta = JSON.parse(event.data) as StreamMeta;
							if (meta.resume_path) resumePath = meta.resume_path;
							handlers.onMeta?.(meta);
						} catch {
						}
					} else if (event.event === 'delta') handlers.onDelta?.(event.data);
					else if (event.event === 'thinking') {
						try {
							handlers.onThinking?.((JSON.parse(event.data) as { text: string }).text);
						} catch {}
					} else if (event.event === 'tool_call') {
						try {
							handlers.onToolCall?.(JSON.parse(event.data) as ToolCall[]);
						} catch {
						}
					} else if (event.event === 'tool_result') {
						try {
							handlers.onToolResult?.(JSON.parse(event.data) as ToolResult);
						} catch {
						}
					} else if (event.event === 'error') handlers.onError?.(new Error(event.data));
					else if (event.event === 'done') {
						handlers.onDone?.();
						return 'done';
					}
				}
			}
		} catch (err) {
			return isAbort(err) ? 'aborted' : 'dropped';
		}
		return 'dropped';
	};

	for (let attempt = 0; ; ) {
		const outcome = await consume(res);
		if (outcome !== 'dropped' || signal?.aborted) return;
		if (!resumePath || attempt >= RECONNECT_ATTEMPTS) {
			handlers.onError?.(new Error('stream interrupted'));
			return;
		}
		attempt++;
		handlers.onReconnecting?.(attempt);
		await delay(RECONNECT_BASE_DELAY_MS * attempt, signal);
		if (signal?.aborted) return;
		try {
			let next = await resume(resumePath, lastEventId);
			if (next.status === 401 && auth.refreshToken && (await refreshOnce())) {
				next = await resume(resumePath, lastEventId);
			}
			if (!next.ok || !next.body) continue;
			res = next;
			handlers.onReconnected?.();
		} catch (err) {
			if (isAbort(err)) return;
		}
	}
}

function delay(ms: number, signal?: AbortSignal): Promise<void> {
	return new Promise((resolve) => {
		const timer = setTimeout(resolve, ms);
		signal?.addEventListener(
			'abort',
			() => {
				clearTimeout(timer);
				resolve();
			},
			{ once: true }
		);
	});
}

function parseFrame(frame: string): { event: string; data: string; id: number | null } | null {
	let event = 'message';
	let id: number | null = null;
	const data: string[] = [];

	for (const line of frame.split(/\r?\n/)) {
		if (line === '' || line.startsWith(':')) continue;
		const colon = line.indexOf(':');
		const field = colon === -1 ? line : line.slice(0, colon);
		let value = colon === -1 ? '' : line.slice(colon + 1);
		if (value.startsWith(' ')) value = value.slice(1);
		if (field === 'event') event = value;
		else if (field === 'data') data.push(value);
		else if (field === 'id') {
			const parsed = Number.parseInt(value, 10);
			if (!Number.isNaN(parsed)) id = parsed;
		}
	}

	if (data.length === 0 && event === 'message') return null;
	return { event, data: data.join('\n'), id };
}

function isAbort(err: unknown): boolean {
	return err instanceof DOMException && err.name === 'AbortError';
}
