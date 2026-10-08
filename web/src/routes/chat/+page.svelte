<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import type { Provider } from '#lib/api';
	import { auth } from '#lib/auth.svelte';
	import { chat } from '#lib/chat.svelte';

	let input = $state('');
	let uploading = $derived(chat.uploading);
	let scroller: HTMLDivElement;
	let fileInput: HTMLInputElement;

	onMount(() => {
		if (!auth.session) goto('/login');
	});

	$effect(() => {
		// Re-run on any message mutation so the view stays pinned to the bottom.
		chat.revision;
		chat.selectedId;
		for (const msg of chat.messages) void msg.content;
		void scrollToBottom();
	});

	async function send() {
		const content = input;
		input = '';
		await chat.send(content);
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

	async function onFile(event: Event) {
		const target = event.target as HTMLInputElement;
		const file = target.files?.[0];
		target.value = '';
		if (file) await chat.upload(file);
	}

	function setProviderKey(provider: Provider) {
		const key = prompt(`API key for "${provider.name}":`);
		if (key === null) return;
		void chat.setProviderKey(provider.id, key.trim());
	}

	function deleteCurrent() {
		const conversation = chat.selected;
		if (!conversation) return;
		if (confirm(`Delete "${conversation.title}"? This cannot be undone.`)) {
			void chat.remove(conversation.id);
		}
	}
</script>

<section class="chat">
	<header class="chat-head">
		<h1>{chat.selected?.title ?? 'New chat'}</h1>
		{#if chat.selectedAgent}
			<span class="agent-tag" title="Agent">{chat.selectedAgent.name}</span>
		{/if}
		{#if chat.sourcesCount > 0}
			<span class="sources">{chat.sourcesCount} source{chat.sourcesCount === 1 ? '' : 's'}</span>
		{/if}
		{#if chat.selected}
			<button class="icon-btn danger" onclick={deleteCurrent} title="Delete chat" aria-label="Delete chat">
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<polyline points="3 6 5 6 21 6" />
					<path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
				</svg>
			</button>
		{/if}
	</header>

	<div class="messages" bind:this={scroller}>
		{#if chat.messages.length === 0 && !chat.loading}
			<div class="empty">
				<span class="mark"></span>
				<p>Send a message to start the conversation.</p>
			</div>
		{/if}
		{#each chat.messages as msg (msg.id)}
			<div class="msg {msg.role}">
				<div class="avatar" aria-hidden="true">{msg.role === 'user' ? 'You' : 'AI'}</div>
				<div class="bubble">
					<div class="content">{msg.content || (chat.streaming ? '…' : '')}</div>
				</div>
			</div>
		{/each}
	</div>

	{#if chat.error}
		<p class="error">{chat.error}</p>
	{/if}
	{#if chat.providers.length === 0}
		<p class="notice">
			No model provider configured. <a href="/setup">Set one up</a> to start chatting.
		</p>
	{:else if !chat.hasUsableProvider}
		<p class="notice">
			No provider key is configured for your account.
			{#if chat.providersNeedingKey.length > 0}
				<button class="link" onclick={() => setProviderKey(chat.providersNeedingKey[0])}>
					Add your {chat.providersNeedingKey[0].name} key
				</button>
			{/if}
		</p>
	{/if}
	{#if chat.modelsError && chat.hasUsableProvider}
		<p class="error">
			Could not load models from your provider: {chat.modelsError}
		</p>
	{/if}
	{#if chat.notice}
		<p class="notice">{chat.notice}</p>
	{/if}

	<form class="composer" onsubmit={(e) => (e.preventDefault(), void send())}>
		<textarea
			bind:value={input}
			onkeydown={onKeydown}
			placeholder="Message…  (Enter to send, Shift+Enter for newline)"
			rows="2"
		></textarea>
		<div class="controls">
			<input bind:this={fileInput} type="file" hidden onchange={onFile} />
			<button
				type="button"
				class="ghost"
				onclick={() => fileInput?.click()}
				disabled={uploading || !chat.selectedId}
			>
				{uploading ? 'Uploading…' : 'Attach'}
			</button>
			{#if chat.agents.length > 0}
				<label class="field" title="Agent for this conversation">
					<span>Agent</span>
					<select
						class="agent"
						value={chat.selected?.agent_id ?? chat.pendingAgentId ?? ''}
						onchange={(e) =>
							void chat.setAgent((e.currentTarget as HTMLSelectElement).value || null)}
					>
						<option value="">No agent</option>
						{#each chat.agents as agent (agent.id)}
							<option value={agent.id}>{agent.name}</option>
						{/each}
					</select>
				</label>
			{/if}
			<label class="field" title="Model served by your provider">
				<span>Model</span>
				{#if chat.models.length > 0}
					<select class="model" bind:value={chat.model}>
						{#each chat.models as model (model.id)}
							<option value={model.id}>{model.id}</option>
						{/each}
					</select>
				{:else}
					<input class="model" bind:value={chat.model} placeholder="model id" />
				{/if}
			</label>
			<button class="primary" type="submit" disabled={chat.streaming || !input.trim()}>
				{chat.streaming ? 'Streaming…' : 'Send'}
			</button>
		</div>
	</form>
</section>

<style>
	.chat {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
		background: var(--bg);
	}
	.chat-head {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		padding: 0.75rem 1.1rem;
		border-bottom: 1px solid var(--border);
		background: color-mix(in srgb, var(--bg-elevated) 70%, transparent);
		backdrop-filter: blur(10px);
	}
	.chat-head h1 {
		margin: 0;
		font-size: 0.98rem;
		font-weight: 600;
		letter-spacing: -0.01em;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.sources {
		margin-left: auto;
		color: var(--text-muted);
		font-size: 0.75rem;
	}
	.agent-tag {
		flex: 0 0 auto;
		padding: 0.15rem 0.55rem;
		border-radius: var(--radius-full);
		background: var(--primary-soft);
		border: 1px solid color-mix(in srgb, var(--primary) 45%, var(--border));
		color: var(--text);
		font-size: 0.72rem;
	}
	.icon-btn {
		display: grid;
		place-items: center;
		width: 34px;
		height: 34px;
		margin-left: auto;
		border: none;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-muted);
	}
	.sources + .icon-btn {
		margin-left: 0.25rem;
	}
	.icon-btn:hover {
		background: var(--surface-hover);
		color: var(--text);
	}
	.icon-btn.danger:hover {
		color: var(--danger);
		background: var(--danger-bg);
	}
	.icon-btn svg {
		width: 17px;
		height: 17px;
	}

	.messages {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		padding: 1.5rem clamp(1rem, 6vw, 4rem);
		display: flex;
		flex-direction: column;
		gap: 1.1rem;
	}
	.msg {
		display: flex;
		gap: 0.75rem;
		max-width: 80%;
		animation: rise 220ms var(--ease) both;
	}
	.msg.user {
		flex-direction: row-reverse;
		align-self: flex-end;
	}
	.avatar {
		flex: 0 0 auto;
		width: 34px;
		height: 34px;
		border-radius: var(--radius-full);
		display: grid;
		place-items: center;
		font-size: 0.7rem;
		font-weight: 700;
		background: var(--surface-active);
		color: var(--text-muted);
		border: 1px solid var(--border);
	}
	.msg.user .avatar {
		background: linear-gradient(135deg, var(--primary-hover), var(--primary-active));
		color: var(--on-primary);
		border-color: transparent;
	}
	.bubble {
		background: var(--surface);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		padding: 0.7rem 0.95rem;
		box-shadow: var(--shadow-sm);
	}
	.msg.user .bubble {
		background: var(--primary-soft);
		border-color: color-mix(in srgb, var(--primary) 45%, var(--border));
	}
	.content {
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}

	.composer {
		border-top: 1px solid var(--border);
		padding: 0.85rem clamp(1rem, 6vw, 4rem);
		display: flex;
		flex-direction: column;
		gap: 0.6rem;
		background: color-mix(in srgb, var(--surface) 45%, transparent);
	}
	textarea {
		resize: vertical;
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius);
		color: var(--text);
		padding: 0.7rem 0.85rem;
		min-height: 52px;
	}
	textarea::placeholder {
		color: var(--text-faint);
	}
	.controls {
		display: flex;
		gap: 0.5rem;
		align-items: center;
		justify-content: flex-end;
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 0.15rem;
		min-width: 0;
		font-size: 0.66rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--text-faint);
	}
	.field:has(.model) {
		flex: 1;
	}
	.agent {
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-full);
		color: var(--text);
		padding: 0.45rem 0.7rem;
	}
	.model {
		width: 100%;
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-full);
		color: var(--text);
		padding: 0.45rem 0.9rem;
	}
	.primary {
		background: linear-gradient(135deg, var(--primary-hover), var(--primary-active));
		border: none;
		border-radius: var(--radius-full);
		color: var(--on-primary);
		padding: 0.5rem 1.4rem;
		font-weight: 600;
		box-shadow: var(--shadow-sm);
	}
	.primary:hover:not(:disabled) {
		box-shadow: var(--shadow-glow);
		filter: brightness(1.06);
	}
	.primary:disabled {
		opacity: 0.45;
		cursor: default;
		box-shadow: none;
	}
	.ghost {
		background: transparent;
		border: 1px solid var(--border-strong);
		color: var(--text);
		border-radius: var(--radius-full);
		padding: 0.45rem 0.95rem;
	}
	.ghost:hover:not(:disabled) {
		background: var(--surface-hover);
		border-color: var(--primary);
	}
	.ghost:disabled {
		opacity: 0.45;
		cursor: default;
	}
	.error {
		color: var(--danger);
		background: var(--danger-bg);
		border: 1px solid color-mix(in srgb, var(--danger) 40%, transparent);
		border-radius: var(--radius-sm);
		margin: 0 clamp(1rem, 6vw, 4rem);
		padding: 0.45rem 0.7rem;
		font-size: 0.85rem;
	}
	.notice {
		color: var(--text-muted);
		margin: 0;
		padding: 0 clamp(1rem, 6vw, 4rem);
		font-size: 0.8rem;
	}
	.link {
		background: none;
		border: none;
		color: var(--accent);
		padding: 0;
		font: inherit;
		font-weight: 600;
		text-decoration: underline;
	}
	.link:hover {
		color: var(--primary-hover);
	}
	.empty {
		margin: auto;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 0.75rem;
		color: var(--text-muted);
	}
	.empty .mark {
		width: 44px;
		height: 44px;
		border-radius: var(--radius);
		background: linear-gradient(135deg, var(--primary-hover), var(--primary-active));
		box-shadow: var(--shadow-glow);
	}
	.empty p {
		margin: 0;
	}
</style>
