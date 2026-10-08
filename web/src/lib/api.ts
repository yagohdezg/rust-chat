import { auth, type Session } from './auth.svelte';

export const API_BASE =
	(import.meta.env.VITE_API_BASE as string | undefined) ?? 'http://localhost:3080';

export type { Session };
export type Conversation = {
	id: string;
	user_id: string;
	agent_id?: string | null;
	title: string;
	created_at: string;
	updated_at: string;
};

export type Message = {
	id: string;
	conversation_id: string;
	role: string;
	content?: string | null;
	tool_calls?: unknown;
	tool_call_id?: string | null;
	created_at: string;
};

export type ExecResult = {
	exit_code: number;
	stdout: string;
	stderr: string;
	timed_out: boolean;
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
	rag_enabled: boolean;
	chunks_indexed: number;
	indexing_error: string | null;
};

export class ApiError extends Error {
	status: number;
	constructor(status: number, message: string) {
		super(message);
		this.name = 'ApiError';
		this.status = status;
	}
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
	const headers = new Headers(init.headers);
	if (init.body && !headers.has('content-type')) headers.set('content-type', 'application/json');
	const token = auth.token;
	if (token) headers.set('authorization', `Bearer ${token}`);

	const res = await fetch(`${API_BASE}${path}`, { ...init, headers });
	if (!res.ok) throw await toApiError(res);
	if (res.status === 204) return undefined as T;
	return (await res.json()) as T;
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

export function listConversations(): Promise<Conversation[]> {
	return request<Conversation[]>('/api/conversations');
}

export function createConversation(title: string): Promise<Conversation> {
	return request<Conversation>('/api/conversations', {
		method: 'POST',
		body: JSON.stringify({ title })
	});
}

export function listMessages(conversationId: string): Promise<Message[]> {
	return request<Message[]>(`/api/conversations/${conversationId}/messages`);
}

export function runSandbox(input: SandboxRequest): Promise<ExecResult> {
	return request<ExecResult>('/api/sandbox/run', {
		method: 'POST',
		body: JSON.stringify(input)
	});
}

export function uploadFile(input: UploadInput): Promise<UploadResponse> {
	return request<UploadResponse>('/api/files', {
		method: 'POST',
		body: JSON.stringify(input)
	});
}

export function listFiles(conversationId: string): Promise<FileRecord[]> {
	return request<FileRecord[]>(`/api/conversations/${conversationId}/files`);
}

export type SourceRef = { file_id: string | null; score: number };

export type StreamHandlers = {
	onSources?: (sources: SourceRef[]) => void;
	onDelta?: (text: string) => void;
	onError?: (error: unknown) => void;
	onDone?: () => void;
};

export type ChatRequest = {
	conversation_id: string;
	content: string;
	model?: string;
};

/**
 * Stream a completion from `POST /api/chat`.
 *
 * `EventSource` cannot issue a POST, so we read the `text/event-stream` body
 * directly and parse SSE frames (`event:` / `data:`), joining multi-line
 * `data:` payloads as the spec requires.
 */
export async function streamChat(
	input: ChatRequest,
	handlers: StreamHandlers,
	signal?: AbortSignal
): Promise<void> {
	const headers = new Headers({ 'content-type': 'application/json' });
	const token = auth.token;
	if (token) headers.set('authorization', `Bearer ${token}`);

	let res: Response;
	try {
		res = await fetch(`${API_BASE}/api/chat`, {
			method: 'POST',
			headers,
			body: JSON.stringify(input),
			signal
		});
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

	const reader = res.body.getReader();
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
				if (event.event === 'sources') {
					try {
						handlers.onSources?.(JSON.parse(event.data) as SourceRef[]);
					} catch {
						// ignore malformed sources payloads
					}
				} else if (event.event === 'delta') handlers.onDelta?.(event.data);
				else if (event.event === 'error') handlers.onError?.(new Error(event.data));
				else if (event.event === 'done') {
					handlers.onDone?.();
					return;
				}
			}
		}
		handlers.onDone?.();
	} catch (err) {
		if (!isAbort(err)) handlers.onError?.(err);
	}
}

function parseFrame(frame: string): { event: string; data: string } | null {
	let event = 'message';
	const data: string[] = [];

	for (const line of frame.split(/\r?\n/)) {
		if (line === '' || line.startsWith(':')) continue;
		const colon = line.indexOf(':');
		const field = colon === -1 ? line : line.slice(0, colon);
		let value = colon === -1 ? '' : line.slice(colon + 1);
		if (value.startsWith(' ')) value = value.slice(1);
		if (field === 'event') event = value;
		else if (field === 'data') data.push(value);
	}

	if (data.length === 0 && event === 'message') return null;
	return { event, data: data.join('\n') };
}

function isAbort(err: unknown): boolean {
	return err instanceof DOMException && err.name === 'AbortError';
}
