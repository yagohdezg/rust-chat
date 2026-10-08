<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import * as api from '#lib/api';
	import { auth } from '#lib/auth.svelte';

	let conversations = $state<api.Conversation[]>([]);
	let selected = $state<string | null>(null);
	let messages = $state<api.Message[]>([]);
	let input = $state('');
	let model = $state('gpt-4o-mini');
	let streaming = $state(false);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let uploading = $state(false);
	let files = $state<api.FileRecord[]>([]);
	let notice = $state<string | null>(null);
	let sourcesCount = $state(0);
	let scroller: HTMLDivElement;
	let fileInput: HTMLInputElement;

	onMount(async () => {
		if (!auth.session) {
			goto('/login');
			return;
		}
		try {
			conversations = await api.listConversations();
			if (conversations.length > 0) await select(conversations[0].id);
		} catch (err) {
			error = message(err);
		} finally {
			loading = false;
		}
	});

	async function select(id: string) {
		selected = id;
		error = null;
		notice = null;
		try {
			messages = await api.listMessages(id);
			files = await api.listFiles(id);
			await scrollToBottom();
		} catch (err) {
			error = message(err);
		}
	}

	async function upload(event: Event) {
		const input = event.target as HTMLInputElement;
		const file = input.files?.[0];
		input.value = '';
		if (!file || !selected) return;

		uploading = true;
		error = null;
		notice = null;
		try {
			const response = await api.uploadFile({
				filename: file.name,
				mime: file.type || undefined,
				content_b64: await fileToBase64(file),
				conversation_id: selected
			});
			files = [...files, response.file];
			if (response.indexing_error) {
				notice = `Attached ${file.name}, but indexing failed: ${response.indexing_error}`;
			} else if (response.rag_enabled) {
				notice = `Attached ${file.name} — indexed ${response.chunks_indexed} chunk(s).`;
			} else {
				notice = `Attached ${file.name} (RAG disabled, not indexed).`;
			}
		} catch (err) {
			error = message(err);
		} finally {
			uploading = false;
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

	async function newConversation(): Promise<api.Conversation> {
		const conversation = await api.createConversation('New chat');
		conversations = [conversation, ...conversations];
		selected = conversation.id;
		messages = [];
		return conversation;
	}

	async function send() {
		const content = input.trim();
		if (!content || streaming || !auth.session) return;

		let conversationId = selected;
		if (!conversationId) {
			try {
				conversationId = (await newConversation()).id;
			} catch (err) {
				error = message(err);
				return;
			}
		}

		input = '';
		error = null;
		messages = [
			...messages,
			localMessage('user', content, conversationId),
			localMessage('assistant', '', conversationId)
		];
		const assistant = messages[messages.length - 1];
		streaming = true;
		sourcesCount = 0;
		await scrollToBottom();

		await api.streamChat(
			{ conversation_id: conversationId, content, model },
			{
				onSources: (sources) => {
					sourcesCount = sources.length;
				},
				onDelta: (text) => {
					assistant.content = (assistant.content ?? '') + text;
					void scrollToBottom();
				},
				onError: (err) => {
					error = message(err);
				},
				onDone: () => {
					streaming = false;
				}
			}
		);
		streaming = false;
	}

	function onKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter' && !event.shiftKey) {
			event.preventDefault();
			void send();
		}
	}

	async function scrollToBottom() {
		await tick();
		scroller?.scrollTo({ top: scroller.scrollHeight });
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
</script>

<div class="layout">
	<aside>
		<button class="new" onclick={() => void newConversation()}>+ New chat</button>
		{#if loading}
			<p class="muted">Loading…</p>
		{:else if conversations.length === 0}
			<p class="muted">No conversations yet.</p>
		{:else}
			<ul>
				{#each conversations as conversation (conversation.id)}
					<li>
						<button
							class:active={conversation.id === selected}
							onclick={() => void select(conversation.id)}
						>
							{conversation.title}
						</button>
					</li>
				{/each}
			</ul>
		{/if}
	</aside>

	<section class="chat">
		<div class="messages" bind:this={scroller}>
			{#if messages.length === 0 && !loading}
				<p class="muted center">Send a message to start the conversation.</p>
			{/if}
			{#each messages as msg (msg.id)}
				<div class="msg {msg.role}">
					<div class="role">{msg.role}</div>
					<div class="content">{msg.content || (streaming ? '…' : '')}</div>
				</div>
			{/each}
		</div>

		{#if error}
			<p class="error">{error}</p>
		{/if}
		{#if notice}
			<p class="notice">{notice}</p>
		{/if}
		{#if files.length}
			<div class="attachments">
				{#each files as file (file.id)}
					<span class="chip" title={file.id}>{file.filename}</span>
				{/each}
			</div>
		{/if}

		<form class="composer" onsubmit={(e) => (e.preventDefault(), void send())}>
			<textarea
				bind:value={input}
				onkeydown={onKeydown}
				placeholder="Message…  (Enter to send, Shift+Enter for newline)"
				rows="2"
			></textarea>
			<div class="controls">
				{#if sourcesCount > 0}
					<span class="sources">{sourcesCount} source{sourcesCount === 1 ? '' : 's'}</span>
				{/if}
				<input bind:this={fileInput} type="file" hidden onchange={upload} />
				<button
					type="button"
					class="ghost"
					onclick={() => fileInput?.click()}
					disabled={uploading || !selected}
				>
					{uploading ? 'Uploading…' : 'Attach'}
				</button>
				<input class="model" bind:value={model} aria-label="model" placeholder="model" />
				<button class="primary" type="submit" disabled={streaming || !input.trim()}>
					{streaming ? 'Streaming…' : 'Send'}
				</button>
			</div>
		</form>
	</section>
</div>

<style>
	.layout {
		display: grid;
		grid-template-columns: 240px 1fr;
		gap: 1rem;
		height: calc(100vh - 120px);
	}
	aside {
		border: 1px solid #1e2430;
		border-radius: 12px;
		background: #0e1118;
		padding: 0.75rem;
		overflow-y: auto;
	}
	.new {
		width: 100%;
		background: #1a2130;
		border: 1px solid #2a3345;
		color: #e6e8ee;
		border-radius: 8px;
		padding: 0.5rem;
		margin-bottom: 0.75rem;
	}
	.new:hover {
		background: #222c3d;
	}
	ul {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
	}
	li button {
		width: 100%;
		text-align: left;
		background: transparent;
		border: none;
		color: #cfd6e4;
		padding: 0.45rem 0.55rem;
		border-radius: 6px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	li button:hover {
		background: #1a2130;
	}
	li button.active {
		background: #22304a;
		color: #fff;
	}
	.chat {
		display: flex;
		flex-direction: column;
		min-height: 0;
		border: 1px solid #1e2430;
		border-radius: 12px;
		background: #0e1118;
	}
	.messages {
		flex: 1;
		overflow-y: auto;
		padding: 1rem;
		display: flex;
		flex-direction: column;
		gap: 0.9rem;
	}
	.msg {
		display: grid;
		grid-template-columns: 72px 1fr;
		gap: 0.75rem;
	}
	.role {
		color: #7b8794;
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		padding-top: 0.15rem;
	}
	.content {
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}
	.msg.user .content {
		color: #e6e8ee;
	}
	.msg.assistant .content {
		color: #b8e0ff;
	}
	.composer {
		border-top: 1px solid #1e2430;
		padding: 0.75rem;
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}
	textarea {
		resize: vertical;
		background: #0b0d12;
		border: 1px solid #2a3345;
		border-radius: 8px;
		color: #e6e8ee;
		padding: 0.6rem 0.7rem;
		font: inherit;
	}
	textarea:focus {
		outline: 2px solid #2563eb;
		outline-offset: 1px;
	}
	.controls {
		display: flex;
		gap: 0.5rem;
		justify-content: flex-end;
	}
	.model {
		flex: 1;
		background: #0b0d12;
		border: 1px solid #2a3345;
		border-radius: 8px;
		color: #e6e8ee;
		padding: 0.4rem 0.6rem;
		font: inherit;
	}
	.primary {
		background: #2563eb;
		border: none;
		border-radius: 8px;
		color: #fff;
		padding: 0.45rem 1rem;
		font-weight: 600;
	}
	.primary:disabled {
		opacity: 0.5;
		cursor: default;
	}
	.muted {
		color: #7b8794;
		font-size: 0.85rem;
	}
	.center {
		text-align: center;
		margin: auto;
	}
	.error {
		color: #fca5a5;
		margin: 0;
		padding: 0 0.75rem;
		font-size: 0.85rem;
	}
	.notice {
		color: #9aa4b2;
		margin: 0;
		padding: 0 0.75rem;
		font-size: 0.8rem;
	}
	.attachments {
		display: flex;
		flex-wrap: wrap;
		gap: 0.35rem;
		padding: 0.4rem 0.75rem 0;
	}
	.chip {
		background: #1a2130;
		border: 1px solid #2a3345;
		border-radius: 999px;
		padding: 0.1rem 0.6rem;
		font-size: 0.75rem;
		color: #cfd6e4;
	}
	.sources {
		align-self: center;
		margin-right: auto;
		color: #7b8794;
		font-size: 0.75rem;
	}
	.ghost {
		background: transparent;
		border: 1px solid #2a3345;
		color: #cfd6e4;
		border-radius: 8px;
		padding: 0.4rem 0.8rem;
	}
	.ghost:hover:not(:disabled) {
		background: #1a2130;
	}
	.ghost:disabled {
		opacity: 0.5;
		cursor: default;
	}
</style>
