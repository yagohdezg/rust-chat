<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import type { Provider } from '#lib/api';
	import { auth } from '#lib/auth.svelte';
	import { chat } from '#lib/chat.svelte';
	import ConfirmDialog from '#lib/ConfirmDialog.svelte';
	import ThinkingOrb from '#lib/ThinkingOrb.svelte';

	let input = $state('');
	let uploading = $derived(chat.uploading);
	let scroller: HTMLDivElement;
	let fileInput: HTMLInputElement;

	/** First + last initial from the display name, falling back to the email. */
	function initials(name: string | null | undefined, email: string | undefined): string {
		const parts = (name ?? '').trim().split(/\s+/).filter(Boolean);
		if (parts.length > 0) {
			const first = parts[0][0] ?? '';
			const last = parts.length > 1 ? (parts[parts.length - 1][0] ?? '') : '';
			return (first + last).toUpperCase();
		}
		return (email ?? '?').slice(0, 1).toUpperCase();
	}

	let userInitials = $derived(initials(auth.session?.user.name, auth.session?.user.email));

	// The agent's provider is authoritative; otherwise offer every provider.
	let visibleProviders = $derived(
		chat.agentProviderId
			? chat.providers.filter((p) => p.id === chat.agentProviderId)
			: chat.providers
	);

	function selectModel(providerId: string, modelId: string) {
		chat.setProvider(providerId, modelId);
		showModels = false;
	}

	// ---- provider management (composer popover) ----------------------------
	let showModels = $state(false);
	let modelMenuEl: HTMLDivElement | undefined;
	let showAddProvider = $state(false);

	// Dismiss the picker on an outside click or Escape.
	$effect(() => {
		if (!showModels) return;
		const onPointerDown = (event: MouseEvent) => {
			if (modelMenuEl && !modelMenuEl.contains(event.target as Node)) showModels = false;
		};
		const onKeydown = (event: KeyboardEvent) => {
			if (event.key === 'Escape') showModels = false;
		};
		window.addEventListener('mousedown', onPointerDown);
		window.addEventListener('keydown', onKeydown);
		return () => {
			window.removeEventListener('mousedown', onPointerDown);
			window.removeEventListener('keydown', onKeydown);
		};
	});
	let newName = $state('OpenAI');
	let newKind = $state('openai');
	let newBaseUrl = $state('https://api.openai.com/v1');
	let newKey = $state('');
	let newGlobal = $state(false);
	let savingProvider = $state(false);

	const isAdmin = $derived(auth.session?.user.role === 'admin');

	async function addProvider(event: SubmitEvent) {
		event.preventDefault();
		if (savingProvider || !newName.trim() || !newBaseUrl.trim()) return;
		savingProvider = true;
		await chat.createProvider({
			name: newName.trim(),
			kind: newKind,
			base_url: newBaseUrl.trim(),
			api_key: newKey.trim() || undefined,
			global: isAdmin && newGlobal
		});
		savingProvider = false;
		if (!chat.error) {
			newKey = '';
			newGlobal = false;
			showAddProvider = false;
		}
	}

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

	let confirmDeleteOpen = $state(false);

	function deleteCurrent() {
		if (chat.selected) confirmDeleteOpen = true;
	}

	function confirmDelete() {
		confirmDeleteOpen = false;
		const conversation = chat.selected;
		if (conversation) void chat.remove(conversation.id);
	}

	function removeProvider(provider: Provider) {
		if (confirm(`Delete provider "${provider.name}"?`)) void chat.removeProvider(provider.id);
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
				<div class="avatar" aria-hidden="true">{msg.role === 'user' ? userInitials : 'AI'}</div>
				<div class="bubble">
					{#if msg.role !== 'user' && !msg.content && chat.streaming}
						<span class="thinking" title="Thinking…">
							<ThinkingOrb state="reasoning" size={22} label="Thinking" />
							<span class="thinking-label">Thinking…</span>
						</span>
					{:else}
						<div class="content">{msg.content}</div>
					{/if}
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

	<div class="composer">
		<textarea
			bind:value={input}
			onkeydown={onKeydown}
			placeholder="Message…  (Enter to send, Shift+Enter for newline)"
			rows="2"
		></textarea>
		<div class="controls">
			<div class="controls-left">
				<input bind:this={fileInput} type="file" hidden onchange={onFile} />
				<button
					type="button"
					class="icon-ghost"
					onclick={() => fileInput?.click()}
					disabled={uploading || !chat.selectedId}
					title={uploading ? 'Uploading…' : 'Attach a file'}
					aria-label="Attach a file"
				>
					<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
						<path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />
					</svg>
				</button>

				<div class="model-menu" bind:this={modelMenuEl}>
					<button
						type="button"
						class="model-chip"
						class:open={showModels}
						onclick={() => (showModels = !showModels)}
						title="Choose a provider and model"
					>
						<span class="chip-label">{chat.model || 'Select model'}</span>
						{#if chat.effectiveProvider}
							<span class="chip-provider">{chat.effectiveProvider.name}</span>
						{/if}
						<svg class="chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<polyline points="6 9 12 15 18 9" />
						</svg>
					</button>

					{#if showModels}
						<div class="model-panel">
							<div class="panel-head">
								<span>Provider &amp; model</span>
								<button class="panel-x" onclick={() => (showModels = false)} aria-label="Close">×</button>
							</div>
							{#if chat.providers.length === 0}
								<p class="panel-empty">No providers configured.</p>
							{:else}
								<ul class="provider-groups">
									{#each visibleProviders as provider (provider.id)}
										<li class="provider-group" class:active={provider.id === chat.effectiveProviderId}>
											<div class="provider-row">
												<button
													class="provider-pick"
													onclick={() => chat.setProvider(provider.id)}
													title={provider.base_url}
												>
													<span class="provider-name">{provider.name}</span>
													{#if !provider.user_id}<span class="tag">global</span>{/if}
													{#if provider.has_key === false}<span class="tag needs">key</span>{/if}
												</button>
												{#if !chat.agentProviderId}
													<button
														class="panel-icon"
														onclick={() => setProviderKey(provider)}
														title={provider.user_id === auth.session?.user.id
															? 'Rotate API key'
															: 'Set your API key'}
														aria-label="Set API key"
													>
														<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
															<path d="M21 2l-2 2m-7.61 7.61a5.5 5.5 0 1 1-7.778 7.778 5.5 5.5 0 0 1 7.777-7.777zm0 0L15.5 7.5m0 0l3 3L22 7l-3-3m-3.5 3.5L19 4" />
														</svg>
													</button>
													<button
														class="panel-icon danger"
														onclick={() => removeProvider(provider)}
														title="Delete provider"
														aria-label="Delete provider"
													>
														<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
															<polyline points="3 6 5 6 21 6" />
															<path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
														</svg>
													</button>
												{/if}
											</div>
											{#if provider.id === chat.effectiveProviderId}
												{#if (chat.modelsByProvider[provider.id] ?? []).length > 0}
													<ul class="model-list">
														{#each chat.modelsByProvider[provider.id] as model (model.id)}
															<li>
																<button
																	class="model-option"
																	class:active={model.id === chat.model}
																	onclick={() => selectModel(provider.id, model.id)}
																>
																	{model.id}
																</button>
															</li>
														{/each}
													</ul>
												{:else}
													<p class="panel-empty">No models loaded.</p>
												{/if}
											{/if}
										</li>
									{/each}
								</ul>
							{/if}
							{#if !chat.agentProviderId}
								{#if showAddProvider}
									<form class="provider-add" onsubmit={addProvider}>
										<input bind:value={newName} placeholder="Label" required />
										<select bind:value={newKind}>
											<option value="openai">OpenAI</option>
											<option value="custom">OpenAI-compatible</option>
										</select>
										<input bind:value={newBaseUrl} placeholder="https://api.openai.com/v1" required />
										<input type="password" bind:value={newKey} placeholder="API key (optional)" autocomplete="off" />
										{#if isAdmin}
											<label class="check">
												<input type="checkbox" bind:checked={newGlobal} /> Share with every user
											</label>
										{/if}
										<button
											type="submit"
											disabled={savingProvider || !newName.trim() || !newBaseUrl.trim()}
										>
											{savingProvider ? 'Saving…' : 'Add provider'}
										</button>
									</form>
								{:else}
									<button class="add-toggle" onclick={() => (showAddProvider = true)}>
										+ Add provider
									</button>
								{/if}
							{/if}
						</div>
					{/if}
				</div>
			</div>

			<div class="controls-right">
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
				{#if chat.agentProviderId}
					<span class="pinned" title="Provider pinned by the selected agent">
						{chat.effectiveProvider?.name ?? 'agent provider'}
					</span>
				{/if}
				<button
					class="primary"
					type="button"
					onclick={() => void send()}
					disabled={chat.streaming || !input.trim()}
				>
					{chat.streaming ? 'Streaming…' : 'Send'}
				</button>
			</div>
		</div>
	</div>

	<ConfirmDialog
		open={confirmDeleteOpen}
		title="Delete chat?"
		message={chat.selected
			? `"${chat.selected.title}" and its messages will be removed. This cannot be undone.`
			: ''}
		confirmLabel="Delete"
		danger
		onconfirm={confirmDelete}
		oncancel={() => (confirmDeleteOpen = false)}
	/>
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
		align-items: flex-start;
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
		/* Nudge down so the ball is centred on the bubble's first text line. */
		margin-top: 0.35rem;
		border-radius: var(--radius-full);
		display: grid;
		place-items: center;
		font-size: 0.7rem;
		font-weight: 700;
		letter-spacing: 0.02em;
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
		justify-content: space-between;
	}
	.controls-left,
	.controls-right {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		min-width: 0;
	}
	.controls-right {
		margin-left: auto;
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
	.agent {
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-full);
		color: var(--text);
		padding: 0.45rem 0.7rem;
	}
	.icon-ghost {
		display: grid;
		place-items: center;
		width: 38px;
		height: 38px;
		flex: 0 0 auto;
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-full);
		background: transparent;
		color: var(--text-muted);
	}
	.icon-ghost:hover:not(:disabled) {
		background: var(--surface-hover);
		border-color: var(--primary);
		color: var(--text);
	}
	.icon-ghost:disabled {
		opacity: 0.45;
		cursor: default;
	}
	.icon-ghost svg {
		width: 17px;
		height: 17px;
	}
	.pinned {
		align-self: center;
		padding: 0.2rem 0.6rem;
		border-radius: var(--radius-full);
		background: var(--primary-soft);
		border: 1px solid color-mix(in srgb, var(--primary) 45%, var(--border));
		color: var(--text);
		font-size: 0.72rem;
	}
	.thinking {
		display: inline-flex;
		align-items: center;
		gap: 0.5rem;
		padding: 0.1rem 0;
		color: var(--primary);
	}
	.thinking-label {
		color: var(--text-muted);
		font-size: 0.85rem;
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
	/* model + provider picker, merged into one chip inside the composer */
	.model-menu {
		position: relative;
		min-width: 0;
	}
	.model-chip {
		display: flex;
		align-items: center;
		gap: 0.45rem;
		max-width: min(360px, 55vw);
		padding: 0.45rem 0.8rem;
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-full);
		background: transparent;
		color: var(--text);
	}
	.model-chip:hover,
	.model-chip.open {
		background: var(--surface-hover);
		border-color: var(--primary);
	}
	.chip-label {
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.chip-provider {
		color: var(--text-faint);
		font-size: 0.78rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.chip-provider::before {
		content: '·';
		margin-right: 0.45rem;
		color: var(--border-strong);
	}
	.chev {
		width: 15px;
		height: 15px;
		flex: 0 0 auto;
		color: var(--text-faint);
	}
	.model-panel {
		position: absolute;
		left: 0;
		bottom: calc(100% + 0.6rem);
		z-index: 20;
		width: min(360px, 85vw);
		max-height: 60vh;
		overflow-y: auto;
		padding: 0.7rem;
		border: 1px solid var(--border-strong);
		border-radius: var(--radius);
		background: var(--bg-elevated);
		box-shadow: var(--shadow);
		animation: rise 160ms var(--ease) both;
	}
	.panel-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: 0.4rem;
		font-size: 0.72rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-faint);
	}
	.panel-x {
		border: none;
		background: transparent;
		color: var(--text-muted);
		font-size: 1.1rem;
		line-height: 1;
	}
	.panel-x:hover {
		color: var(--text);
	}
	.panel-empty {
		margin: 0.2rem 0.3rem;
		color: var(--text-faint);
		font-size: 0.82rem;
	}
	.provider-groups {
		list-style: none;
		margin: 0 0 0.4rem;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.15rem;
	}
	.provider-group {
		border-radius: var(--radius-sm);
	}
	.provider-group.active {
		background: var(--primary-soft);
	}
	.provider-row {
		display: flex;
		align-items: center;
		gap: 0.15rem;
		border-radius: var(--radius-sm);
	}
	.provider-pick {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: center;
		gap: 0.4rem;
		text-align: left;
		border: none;
		background: transparent;
		color: var(--text-muted);
		padding: 0.45rem 0.5rem;
		overflow: hidden;
	}
	.provider-group.active .provider-pick,
	.provider-pick:hover {
		color: var(--text);
	}
	.model-list {
		list-style: none;
		margin: 0.1rem 0 0.35rem;
		padding: 0 0 0 0.55rem;
		display: flex;
		flex-direction: column;
		gap: 0.05rem;
	}
	.model-option {
		width: 100%;
		text-align: left;
		border: none;
		background: transparent;
		color: var(--text-muted);
		padding: 0.4rem 0.6rem;
		border-radius: var(--radius-sm);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.model-option:hover {
		background: var(--surface-hover);
		color: var(--text);
	}
	.model-option.active {
		background: var(--primary);
		color: var(--on-primary);
	}
	.add-toggle {
		width: 100%;
		margin-top: 0.5rem;
		padding: 0.45rem;
		border: 1px dashed var(--border-strong);
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-muted);
		text-align: center;
	}
	.add-toggle:hover {
		color: var(--text);
		border-color: var(--primary);
	}
	.provider-name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.tag {
		flex: 0 0 auto;
		padding: 0 0.35rem;
		border-radius: var(--radius-full);
		background: var(--surface-active);
		color: var(--text-faint);
		font-size: 0.62rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}
	.tag.needs {
		background: var(--danger-bg);
		color: var(--danger);
	}
	.panel-icon {
		display: grid;
		place-items: center;
		width: 28px;
		height: 28px;
		flex: 0 0 auto;
		border: none;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-faint);
	}
	.panel-icon:hover {
		background: var(--surface-hover);
		color: var(--text);
	}
	.panel-icon.danger:hover {
		color: var(--danger);
		background: var(--danger-bg);
	}
	.panel-icon svg {
		width: 15px;
		height: 15px;
	}
	.provider-add {
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
		padding-top: 0.5rem;
		border-top: 1px solid var(--border);
	}
	.provider-add input:not([type='checkbox']),
	.provider-add select {
		width: 100%;
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-sm);
		color: var(--text);
		padding: 0.45rem 0.6rem;
		font-size: 0.82rem;
	}
	.provider-add .check {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		color: var(--text-muted);
		font-size: 0.8rem;
	}
	.provider-add button[type='submit'] {
		border: none;
		border-radius: var(--radius-sm);
		background: var(--primary);
		color: var(--on-primary);
		padding: 0.45rem;
		font-weight: 600;
	}
	.provider-add button[type='submit']:disabled {
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
