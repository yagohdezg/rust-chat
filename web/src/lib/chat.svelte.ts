import * as api from './api';
import { auth } from './auth.svelte';

/**
 * Shared chat controller. Lives in a `.svelte.ts` module so the sidebar (in the
 * root layout) and the chat page read and mutate the same runes state.
 */
class ChatStore {
	conversations = $state<api.Conversation[]>([]);
	agents = $state<api.Agent[]>([]);
	providers = $state<api.Provider[]>([]);
	/** Model catalogs keyed by provider id. */
	modelsByProvider = $state<Record<string, api.ModelInfo[]>>({});
	/** Per-provider catalog load failures, keyed by provider id. */
	providerErrors = $state<Record<string, string>>({});
	selectedId = $state<string | null>(null);
	/** Explicitly chosen provider for the next turn; empty means "auto". */
	providerId = $state<string>('');
	/** Agent bound to the *next* conversation; applied when a chat is created. */
	pendingAgentId = $state<string | null>(null);
	messages = $state<api.Message[]>([]);
	/** Files attached to the selected conversation. */
	files = $state<api.FileRecord[]>([]);
	/** The caller's whole personal library (for the attach picker). */
	library = $state<api.FileRecord[]>([]);
	libraryLoaded = $state(false);
	libraryLoading = $state(false);
	loading = $state(false);
	initialized = $state(false);
	streaming = $state(false);
	uploading = $state(false);
	error = $state<string | null>(null);
	notice = $state<string | null>(null);
	model = $state('');
	/** Bumped whenever messages change, so views can react (e.g. auto-scroll). */
	revision = $state(0);

	/** True once loaded and no provider exists yet — the app needs setup. */
	get needsSetup(): boolean {
		return this.initialized && !this.loading && this.providers.length === 0;
	}

	/** True when at least one provider has a key usable by this user. */
	get hasUsableProvider(): boolean {
		return this.providers.some((p) => p.has_key !== false);
	}

	/** Providers the user must still supply a key for. */
	get providersNeedingKey(): api.Provider[] {
		return this.providers.filter((p) => p.has_key === false);
	}

	/** Provider pinned by the bound (or pending) agent, if any. */
	get agentProviderId(): string | null {
		return this.selectedAgent?.provider_id ?? this.pendingAgent?.provider_id ?? null;
	}

	/**
	 * Provider that will actually serve the next turn: the agent's if bound,
	 * else the user's explicit pick, else the deployment default (first row).
	 */
	get effectiveProviderId(): string {
		return this.agentProviderId ?? (this.providerId || this.providers[0]?.id || '');
	}

	get effectiveProvider(): api.Provider | null {
		return this.providers.find((p) => p.id === this.effectiveProviderId) ?? null;
	}

	/** Models offered by the effective provider. */
	get models(): api.ModelInfo[] {
		return this.modelsByProvider[this.effectiveProviderId] ?? [];
	}

	/** Catalog error for the effective provider, if its refresh failed. */
	get modelsError(): string | null {
		return this.providerErrors[this.effectiveProviderId] ?? null;
	}

	get selected(): api.Conversation | null {
		return this.conversations.find((c) => c.id === this.selectedId) ?? null;
	}

	/** Agent bound to the selected conversation, if any. */
	get selectedAgent(): api.Agent | null {
		const id = this.selected?.agent_id;
		return id ? (this.agents.find((a) => a.id === id) ?? null) : null;
	}

	/** Agent that will be attached when the next conversation is created. */
	get pendingAgent(): api.Agent | null {
		return this.pendingAgentId
			? (this.agents.find((a) => a.id === this.pendingAgentId) ?? null)
			: null;
	}

	/** Ids of library files currently attached to the selected conversation. */
	get attachedFileIds(): Set<string> {
		return new Set(this.files.map((f) => f.id));
	}

	private touch() {
		this.revision++;
	}

	async init() {
		if (!auth.session) return;
		this.loading = true;
		this.error = null;
		try {
			[this.conversations, this.agents, this.providers] = await Promise.all([
				api.listConversations(),
				api.listAgents(),
				api.listProviders()
			]);
			if (this.hasUsableProvider) {
				await this.loadModels();
			}
			if (this.conversations.length > 0) {
				await this.select(this.conversations[0].id);
			}
		} catch (err) {
			this.error = message(err);
		} finally {
			this.loading = false;
		}
	}

	/** Refresh every provider's model catalog (best-effort, per provider). */
	async loadModels() {
		const providers = this.providers;
		if (providers.length === 0) {
			this.modelsByProvider = {};
			this.providerErrors = {};
			this.model = '';
			return;
		}
		const results = await Promise.all(
			providers.map(async (provider) => {
				try {
					return { id: provider.id, models: await api.listProviderModels(provider.id), error: null };
				} catch (err) {
					return { id: provider.id, models: [] as api.ModelInfo[], error: message(err) };
				}
			})
		);
		const byProvider: Record<string, api.ModelInfo[]> = {};
		const errors: Record<string, string> = {};
		for (const { id, models, error } of results) {
			byProvider[id] = models;
			if (error) errors[id] = error;
		}
		this.modelsByProvider = byProvider;
		this.providerErrors = errors;
		this.ensureModel();
	}

	/** Keep `model` valid for the effective provider, defaulting to its first. */
	private ensureModel() {
		const models = this.models;
		if (models.length === 0) return;
		if (!models.some((m) => m.id === this.model)) {
			this.model = models[0].id;
		}
	}

	/** Choose the provider (and optionally model) for subsequent turns. */
	setProvider(providerId: string, model?: string) {
		this.providerId = providerId || '';
		if (model !== undefined) this.model = model;
		this.ensureModel();
		this.touch();
	}

	reset() {
		this.initialized = false;
		this.conversations = [];
		this.agents = [];
		this.providers = [];
		this.modelsByProvider = {};
		this.providerErrors = {};
		this.model = '';
		this.providerId = '';
		this.selectedId = null;
		this.pendingAgentId = null;
		this.messages = [];
		this.files = [];
		this.library = [];
		this.libraryLoaded = false;
		this.error = null;
		this.notice = null;
		this.touch();
	}

	async select(id: string) {
		this.selectedId = id;
		this.error = null;
		this.notice = null;
		try {
			const [messages, files] = await Promise.all([api.listMessages(id), api.listFiles(id)]);
			this.messages = messages;
			this.files = files;
			this.ensureModel();
			this.touch();
		} catch (err) {
			this.error = message(err);
		}
	}

	/** Pinned conversations first, then most recently updated. */
	private sortConversations() {
		this.conversations = [...this.conversations].sort(
			(a, b) =>
				Number(b.pinned) - Number(a.pinned) || b.updated_at.localeCompare(a.updated_at)
		);
	}

	async create(agentId: string | null = this.pendingAgentId): Promise<api.Conversation> {
		const conversation = await api.createConversation('New chat', agentId ?? undefined);
		this.conversations = [conversation, ...this.conversations];
		this.sortConversations();
		this.selectedId = conversation.id;
		this.messages = [];
		this.files = [];
		this.touch();
		return conversation;
	}

	/** Start a new conversation bound to `agentId`. */
	async createWithAgent(agentId: string | null) {
		this.pendingAgentId = agentId;
		try {
			await this.create(agentId);
		} catch (err) {
			this.error = message(err);
		}
	}

	/** Rename a conversation; returns false when the new title is unusable. */
	async rename(id: string, title: string): Promise<boolean> {
		const trimmed = title.trim();
		if (!trimmed) return false;
		this.error = null;
		try {
			const updated = await api.renameConversation(id, trimmed);
			this.conversations = this.conversations.map((c) => (c.id === updated.id ? updated : c));
			this.touch();
			return true;
		} catch (err) {
			this.error = message(err);
			return false;
		}
	}

	/** Pin or unpin a conversation, keeping the pinned-first ordering. */
	async setPinned(id: string, pinned: boolean) {
		this.error = null;
		try {
			const updated = await api.setConversationPinned(id, pinned);
			this.conversations = this.conversations.map((c) => (c.id === updated.id ? updated : c));
			this.sortConversations();
			this.touch();
		} catch (err) {
			this.error = message(err);
		}
	}

	/** Deep-copy a conversation (messages included) and select the copy. */
	async duplicate(id: string): Promise<api.Conversation | null> {
		this.error = null;
		try {
			const copy = await api.duplicateConversation(id);
			this.conversations = [copy, ...this.conversations];
			this.sortConversations();
			await this.select(copy.id);
			return copy;
		} catch (err) {
			this.error = message(err);
			return null;
		}
	}

	/** Rebind the selected conversation (or the next one) to `agentId`. */
	async setAgent(agentId: string | null) {
		if (!this.selectedId) {
			this.pendingAgentId = agentId;
			return;
		}
		this.error = null;
		try {
			const updated = await api.setConversationAgent(this.selectedId, agentId);
			this.conversations = this.conversations.map((c) => (c.id === updated.id ? updated : c));
			this.ensureModel();
			this.touch();
		} catch (err) {
			this.error = message(err);
		}
	}

	async createAgent(input: api.CreateAgentInput): Promise<api.Agent | null> {
		this.error = null;
		try {
			const agent = await api.createAgent(input);
			this.agents = [...this.agents, agent];
			this.touch();
			return agent;
		} catch (err) {
			this.error = message(err);
			return null;
		}
	}

	async removeAgent(id: string) {
		this.error = null;
		try {
			await api.deleteAgent(id);
		} catch (err) {
			this.error = message(err);
			return;
		}
		this.agents = this.agents.filter((a) => a.id !== id);
		if (this.pendingAgentId === id) this.pendingAgentId = null;
		this.touch();
	}

	async createProvider(input: api.CreateProviderInput): Promise<api.Provider | null> {
		this.error = null;
		try {
			const provider = await api.createProvider(input);
			this.providers = [...this.providers, provider];
			await this.loadModels();
			this.touch();
			return provider;
		} catch (err) {
			this.error = message(err);
			return null;
		}
	}

	async removeProvider(id: string) {
		this.error = null;
		try {
			await api.deleteProvider(id);
		} catch (err) {
			this.error = message(err);
			return;
		}
		this.providers = this.providers.filter((p) => p.id !== id);
		await this.loadModels();
		this.touch();
	}

	/**
	 * Set a provider's key and refresh the model catalog. Updates the shared key
	 * when the provider belongs to the caller, else stores a personal credential
	 * (the admin-provisioned-provider case). An empty string clears the key.
	 */
	async setProviderKey(id: string, apiKey: string) {
		this.error = null;
		this.notice = null;
		const provider = this.providers.find((p) => p.id === id);
		const owns = !!provider && provider.user_id === auth.session?.user.id;
		try {
			const updated = owns
				? await api.updateProvider(id, { api_key: apiKey })
				: await api.setProviderCredential(id, apiKey);
			this.providers = this.providers.map((p) => (p.id === updated.id ? updated : p));
			await this.loadModels();
			this.notice = apiKey
				? `Updated the API key for ${updated.name}.`
				: `Cleared the API key for ${updated.name}.`;
			this.touch();
		} catch (err) {
			this.error = message(err);
		}
	}

	async remove(id: string) {
		const wasSelected = this.selectedId === id;
		try {
			await api.deleteConversation(id);
		} catch (err) {
			this.error = message(err);
			return;
		}
		this.conversations = this.conversations.filter((c) => c.id !== id);
		if (wasSelected) {
			this.selectedId = null;
			this.messages = [];
			this.files = [];
			const next = this.conversations[0];
			if (next) await this.select(next.id);
		}
		this.touch();
	}

	/** Return the active conversation id, creating one when none is selected. */
	private async ensureConversation(): Promise<string | null> {
		if (this.selectedId) return this.selectedId;
		try {
			return (await this.create()).id;
		} catch (err) {
			this.error = message(err);
			return null;
		}
	}

	async upload(file: File) {
		const conversationId = await this.ensureConversation();
		if (!conversationId) return;
		this.uploading = true;
		this.error = null;
		this.notice = null;
		try {
			const response = await api.uploadFile({
				filename: file.name,
				mime: file.type || undefined,
				content_b64: await fileToBase64(file),
				conversation_id: conversationId
			});
			this.files = [...this.files, response.file];
			if (this.libraryLoaded) this.library = [...this.library, response.file];
			this.notice = `Attached ${file.name}.`;
		} catch (err) {
			this.error = message(err);
		} finally {
			this.uploading = false;
		}
	}

	/** Fetch the personal file library (used by the attach picker). */
	async loadLibrary() {
		if (this.libraryLoading) return;
		this.libraryLoading = true;
		this.error = null;
		try {
			this.library = await api.listUserFiles();
			this.libraryLoaded = true;
		} catch (err) {
			this.error = message(err);
		} finally {
			this.libraryLoading = false;
		}
	}

	/** Attach an existing library file to the selected conversation. */
	async attachFile(fileId: string) {
		const conversationId = await this.ensureConversation();
		if (!conversationId) return;
		this.error = null;
		try {
			this.files = await api.attachConversationFiles(conversationId, [fileId]);
			this.touch();
		} catch (err) {
			this.error = message(err);
		}
	}

	/** Detach a file from the selected conversation (it stays in the library). */
	async detachFile(fileId: string) {
		if (!this.selectedId) return;
		this.error = null;
		try {
			await api.detachConversationFile(this.selectedId, fileId);
			this.files = this.files.filter((f) => f.id !== fileId);
			this.touch();
		} catch (err) {
			this.error = message(err);
		}
	}

	/** Delete a file from the library, detaching it from the current chat. */
	async removeLibraryFile(fileId: string) {
		this.error = null;
		try {
			await api.deleteFile(fileId);
			this.library = this.library.filter((f) => f.id !== fileId);
			this.files = this.files.filter((f) => f.id !== fileId);
			this.notice = 'File deleted.';
			this.touch();
		} catch (err) {
			this.error = message(err);
		}
	}

	/** Save a library file to the user's machine. */
	async downloadFile(file: api.FileRecord) {
		this.error = null;
		try {
			await api.downloadFile(file);
		} catch (err) {
			this.error = message(err);
		}
	}

	async send(content: string) {
		const text = content.trim();
		if (!text || this.streaming || !auth.session) return;

		let conversationId = this.selectedId;
		if (!conversationId) {
			try {
				conversationId = (await this.create()).id;
			} catch (err) {
				this.error = message(err);
				return;
			}
		}

		this.error = null;
		this.messages = [
			...this.messages,
			localMessage('user', text, conversationId),
			localMessage('assistant', '', conversationId)
		];
		// Read the proxied element back so streaming mutations stay reactive.
		const assistant = this.messages[this.messages.length - 1];
		this.streaming = true;
		this.touch();

		await api.streamChat(
			{
				conversation_id: conversationId,
				content: text,
				model: this.model,
				provider_id: this.effectiveProviderId || undefined
			},
			{
				onDelta: (delta) => {
					assistant.content = (assistant.content ?? '') + delta;
					this.touch();
				},
				onError: (err) => {
					this.error = message(err);
				},
				onDone: () => {
					this.streaming = false;
				}
			}
		);
		this.streaming = false;
	}
}

async function fileToBase64(file: File): Promise<string> {
	const bytes = new Uint8Array(await file.arrayBuffer());
	let binary = '';
	const chunk = 0x8000;
	for (let i = 0; i < bytes.length; i += chunk) {
		binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
	}
	return btoa(binary);
}

function localMessage(role: string, content: string, conversationId: string): api.Message {
	return {
		id: crypto.randomUUID(),
		conversation_id: conversationId,
		role,
		content,
		created_at: new Date().toISOString()
	};
}

function message(err: unknown): string {
	return err instanceof Error ? err.message : String(err);
}

export const chat = new ChatStore();
