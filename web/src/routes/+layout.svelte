<script lang="ts">
	import '../app.css';
	import favicon from '#lib/assets/favicon.svg';
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import * as api from '#lib/api';
	import { auth } from '#lib/auth.svelte';
	import { chat } from '#lib/chat.svelte';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	const SIDEBAR_KEY = 'rust-chat.sidebar-collapsed';
	let collapsed = $state(false);

	onMount(() => {
		collapsed = localStorage.getItem(SIDEBAR_KEY) === '1';
	});

	// Load conversations when a session appears; clear when it goes away.
	$effect(() => {
		const session = auth.session;
		if (session && !chat.initialized) {
			chat.initialized = true;
			void chat.init();
		} else if (!session && chat.initialized) {
			chat.reset();
		}
	});

	// A fresh instance has no provider: send regular users to setup, but never
	// block an admin — they may need to configure other things first.
	$effect(() => {
		if (
			chat.needsSetup &&
			auth.session?.user.role !== 'admin' &&
			page.url.pathname !== '/setup'
		) {
			goto('/setup');
		}
	});

	$effect(() => {
		if (typeof localStorage !== 'undefined') {
			localStorage.setItem(SIDEBAR_KEY, collapsed ? '1' : '0');
		}
	});

	function expand() {
		collapsed = false;
	}

	/** Selecting/creating a conversation should also show the chat view. */
	function openChat() {
		if (page.url.pathname !== '/chat') goto('/chat');
	}

	function newChat() {
		openChat();
		void chat.create();
	}

	function selectConversation(id: string) {
		openChat();
		void chat.select(id);
	}

	function startWithAgent(id: string) {
		openChat();
		void chat.createWithAgent(id);
	}

	function logout() {
		const refresh = auth.refreshToken;
		auth.clear();
		chat.reset();
		// Best-effort server-side revocation; the local session is gone either way.
		void api.logout(refresh);
		goto('/login');
	}

	function confirmRemove(id: string, title: string) {
		if (confirm(`Delete "${title}"? This cannot be undone.`)) void chat.remove(id);
	}

	let showAgentForm = $state(false);
	let agentName = $state('');
	let agentInstructions = $state('');
	let agentModel = $state('');
	let agentSandbox = $state(false);
	let creatingAgent = $state(false);

	async function submitAgent() {
		const name = agentName.trim();
		if (!name || creatingAgent) return;
		creatingAgent = true;
		const agent = await chat.createAgent({
			name,
			instructions: agentInstructions.trim() || undefined,
			model: agentModel.trim() || undefined,
			sandbox_enabled: agentSandbox
		});
		creatingAgent = false;
		if (agent) {
			agentName = '';
			agentInstructions = '';
			agentModel = '';
			agentSandbox = false;
			showAgentForm = false;
			startWithAgent(agent.id);
		}
	}

	function confirmRemoveAgent(id: string, name: string) {
		if (confirm(`Delete agent "${name}"?`)) void chat.removeAgent(id);
	}

	function confirmRemoveProvider(id: string, name: string) {
		if (confirm(`Delete provider "${name}"?`)) void chat.removeProvider(id);
	}

	function setProviderKey(id: string, name: string) {
		const key = prompt(`API key for "${name}" (leave blank to clear):`);
		if (key === null) return;
		void chat.setProviderKey(id, key.trim());
	}

	function initial(email: string | undefined): string {
		return (email ?? '?').slice(0, 1).toUpperCase();
	}
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<title>rust-chat</title>
</svelte:head>

{#if auth.session}
	<div class="app">
		<aside class="sidebar" class:collapsed>
			<div class="side-head">
				<button
					class="brand"
					onclick={() => (collapsed ? expand() : goto('/chat'))}
					title="rust-chat"
				>
					<span class="mark"></span>
					{#if !collapsed}<span class="brand-name">rust-chat</span>{/if}
				</button>
				{#if !collapsed}
					<button class="icon-btn collapse" onclick={() => (collapsed = true)} title="Collapse">
						<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<polyline points="15 18 9 12 15 6" />
						</svg>
					</button>
				{/if}
			</div>

			<button class="new-chat" onclick={newChat} title="New chat">
				<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
					<line x1="12" y1="5" x2="12" y2="19" />
					<line x1="5" y1="12" x2="19" y2="12" />
				</svg>
				{#if !collapsed}<span>New chat</span>{/if}
			</button>

			{#if collapsed}
				<nav class="rail">
					<button class="rail-btn" onclick={expand} title="Chats">
						<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z" />
						</svg>
					</button>
					<button class="rail-btn" onclick={expand} title="Skills">
						<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<path d="M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9z" />
						</svg>
					</button>
					<button class="rail-btn" onclick={expand} title="Attached files">
						<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />
						</svg>
					</button>
					<button class="rail-btn" onclick={expand} title="MCP servers">
						<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<rect x="3" y="4" width="18" height="7" rx="2" />
							<rect x="3" y="13" width="18" height="7" rx="2" />
							<line x1="7" y1="7.5" x2="7.01" y2="7.5" />
							<line x1="7" y1="16.5" x2="7.01" y2="16.5" />
						</svg>
					</button>
					<a class="rail-btn" href="/sandbox" title="Sandbox">
						<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<polyline points="4 17 10 11 4 5" />
							<line x1="12" y1="19" x2="20" y2="19" />
						</svg>
					</a>
					{#if auth.session.user.role === 'admin'}
						<a class="rail-btn" href="/admin" title="Admin">
							<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
								<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
							</svg>
						</a>
					{/if}
				</nav>
			{:else}
				<div class="sections">
					<section class="section">
						<h2>Chats</h2>
						{#if chat.loading}
							<p class="empty-hint">Loading…</p>
						{:else if chat.conversations.length === 0}
							<p class="empty-hint">No conversations yet.</p>
						{:else}
							<ul class="conv-list">
								{#each chat.conversations as conversation (conversation.id)}
									<li class:active={conversation.id === chat.selectedId}>
											<button
												class="conv"
												onclick={() => selectConversation(conversation.id)}
												title={conversation.title}
											>
											{conversation.title}
										</button>
										<button
											class="conv-del"
											onclick={() => confirmRemove(conversation.id, conversation.title)}
											title="Delete chat"
											aria-label="Delete chat"
										>
											<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
												<polyline points="3 6 5 6 21 6" />
												<path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
											</svg>
										</button>
									</li>
								{/each}
							</ul>
						{/if}
					</section>

					<section class="section">
						<div class="section-head">
							<h2>Agents</h2>
							<button
								class="mini"
								onclick={() => (showAgentForm = !showAgentForm)}
								title={showAgentForm ? 'Cancel' : 'New agent'}
							>
								{showAgentForm ? '×' : '+'}
							</button>
						</div>
						{#if chat.agents.length === 0 && !showAgentForm}
							<p class="empty-hint">No agents yet.</p>
						{:else}
							<ul class="agent-list">
								{#each chat.agents as agent (agent.id)}
									<li class:active={agent.id === chat.pendingAgentId}>
										<button
											class="conv"
											onclick={() => startWithAgent(agent.id)}
											title={agent.instructions ?? agent.name}
										>
											{agent.name}
										</button>
										<button
											class="conv-del"
											onclick={() => confirmRemoveAgent(agent.id, agent.name)}
											title="Delete agent"
											aria-label="Delete agent"
										>
											<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
												<polyline points="3 6 5 6 21 6" />
												<path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
											</svg>
										</button>
									</li>
								{/each}
							</ul>
						{/if}
						{#if showAgentForm}
							<form class="agent-form" onsubmit={(e) => (e.preventDefault(), void submitAgent())}>
								<input bind:value={agentName} placeholder="Name" required />
								<textarea
									bind:value={agentInstructions}
									placeholder="Instructions (system prompt)"
									rows="2"
								></textarea>
								<input bind:value={agentModel} placeholder="Model (optional)" />
								<label class="check">
									<input type="checkbox" bind:checked={agentSandbox} /> Sandbox tool
								</label>
								<button type="submit" disabled={creatingAgent || !agentName.trim()}>
									{creatingAgent ? 'Creating…' : 'Create agent'}
								</button>
							</form>
						{/if}
					</section>

					<section class="section">
						<div class="section-head">
							<h2>Providers</h2>
							<a class="mini" href="/setup" title="Add provider">+</a>
						</div>
						{#if chat.providers.length === 0}
							<p class="empty-hint">No providers configured.</p>
						{:else}
							<ul class="agent-list">
								{#each chat.providers as provider (provider.id)}
									<li>
										<span class="conv provider" title={provider.base_url}>
											{provider.name}
											{#if !provider.user_id}<span class="tag">global</span>{/if}
											{#if provider.has_key === false}<span class="tag needs">key</span>{/if}
										</span>
										<button
											class="conv-del key"
											onclick={() => setProviderKey(provider.id, provider.name)}
											title={provider.user_id === auth.session.user.id
												? 'Rotate API key'
												: 'Set your API key'}
											aria-label="Set API key"
										>
											<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
												<path d="M21 2l-2 2m-7.61 7.61a5.5 5.5 0 1 1-7.778 7.778 5.5 5.5 0 0 1 7.777-7.777zm0 0L15.5 7.5m0 0l3 3L22 7l-3-3m-3.5 3.5L19 4" />
											</svg>
										</button>
										<button
											class="conv-del"
											onclick={() => confirmRemoveProvider(provider.id, provider.name)}
											title="Delete provider"
											aria-label="Delete provider"
										>
											<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
												<polyline points="3 6 5 6 21 6" />
												<path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
											</svg>
										</button>
									</li>
								{/each}
							</ul>
						{/if}
					</section>

					<section class="section">
						<h2>Attached files</h2>
						{#if chat.files.length === 0}
							<p class="empty-hint">No files in this chat.</p>
						{:else}
							<ul class="file-list">
								{#each chat.files as file (file.id)}
									<li class="file-item" title={file.filename}>
										<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
											<path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />
										</svg>
										<span class="file-name">{file.filename}</span>
									</li>
								{/each}
							</ul>
						{/if}
					</section>

					<section class="section">
						<h2>MCP servers</h2>
						<p class="empty-hint">No MCP servers.</p>
					</section>

					<a class="utility" href="/sandbox" class:active={page.url.pathname.startsWith('/sandbox')}>
						<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<polyline points="4 17 10 11 4 5" />
							<line x1="12" y1="19" x2="20" y2="19" />
						</svg>
						<span>Sandbox</span>
					</a>
					{#if auth.session.user.role === 'admin'}
						<a class="utility" href="/admin" class:active={page.url.pathname.startsWith('/admin')}>
							<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
								<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
							</svg>
							<span>Admin</span>
						</a>
					{/if}
				</div>
			{/if}

			<div class="side-foot">
				<button class="avatar" onclick={expand} title={auth.session.user.email}>
					{initial(auth.session.user.email)}
				</button>
				{#if !collapsed}
					<div class="who-text">
						<span class="name">{auth.session.user.name ?? 'Signed in'}</span>
						<span class="email">{auth.session.user.email}</span>
					</div>
					<button class="icon-btn" onclick={logout} title="Sign out">
						<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
							<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
							<polyline points="16 17 21 12 16 7" />
							<line x1="21" y1="12" x2="9" y2="12" />
						</svg>
					</button>
				{/if}
			</div>
		</aside>

		<main>{@render children()}</main>
	</div>
{:else}
	<main class="bare">{@render children()}</main>
{/if}

<style>
	.app {
		display: flex;
		height: 100vh;
		overflow: hidden;
	}

	.sidebar {
		width: 280px;
		flex: 0 0 auto;
		display: flex;
		flex-direction: column;
		background: var(--bg-elevated);
		border-right: 1px solid var(--border);
		overflow: hidden;
		transition: width 200ms var(--ease);
	}
	.sidebar.collapsed {
		width: 68px;
	}

	.side-head {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		padding: 0.8rem 0.8rem 0.4rem;
	}
	.brand {
		display: flex;
		align-items: center;
		gap: 0.55rem;
		flex: 1;
		min-width: 0;
		padding: 0;
		background: none;
		border: none;
		color: var(--text);
		font-weight: 700;
		letter-spacing: -0.02em;
	}
	.brand-name {
		white-space: nowrap;
		overflow: hidden;
	}
	.mark {
		width: 22px;
		height: 22px;
		flex: 0 0 auto;
		border-radius: 7px;
		background: linear-gradient(135deg, var(--primary-hover), var(--primary-active));
		box-shadow: 0 0 0 1px var(--border-strong), 0 3px 10px var(--primary-glow);
	}
	.sidebar.collapsed .side-head {
		justify-content: center;
		padding: 0.8rem 0 0.4rem;
	}

	.icon-btn {
		display: grid;
		place-items: center;
		width: 32px;
		height: 32px;
		flex: 0 0 auto;
		border: none;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-muted);
	}
	.icon-btn:hover {
		background: var(--surface-hover);
		color: var(--text);
	}
	.icon-btn svg {
		width: 18px;
		height: 18px;
	}

	.new-chat {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 0.5rem;
		margin: 0.2rem 0.8rem 0.5rem;
		padding: 0.6rem 0.7rem;
		border: 1px solid transparent;
		border-radius: var(--radius);
		background: linear-gradient(135deg, var(--primary-hover), var(--primary-active));
		color: var(--on-primary);
		font-weight: 600;
		box-shadow: var(--shadow-sm);
	}
	.new-chat:hover {
		box-shadow: var(--shadow-glow);
		filter: brightness(1.06);
	}
	.new-chat svg {
		width: 18px;
		height: 18px;
	}
	.sidebar.collapsed .new-chat {
		width: 40px;
		height: 40px;
		margin: 0.2rem auto 0.5rem;
		padding: 0;
	}

	.rail {
		flex: 1;
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 0.3rem;
		padding: 0.5rem 0;
	}
	.rail-btn {
		display: grid;
		place-items: center;
		width: 40px;
		height: 40px;
		border: none;
		border-radius: var(--radius);
		background: transparent;
		color: var(--text-muted);
	}
	.rail-btn:hover {
		background: var(--surface-hover);
		color: var(--text);
	}
	.rail-btn svg {
		width: 20px;
		height: 20px;
	}

	.sections {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		padding: 0 0.6rem 0.6rem;
	}
	.section {
		margin-top: 1rem;
	}
	.section h2 {
		margin: 0 0 0.35rem;
		padding: 0 0.4rem;
		font-size: 0.7rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-faint);
	}
	.empty-hint {
		margin: 0;
		padding: 0.35rem 0.6rem;
		color: var(--text-faint);
		font-size: 0.82rem;
	}

	.section-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}
	.section-head h2 {
		margin: 0;
	}
	.mini {
		display: grid;
		place-items: center;
		width: 22px;
		height: 22px;
		margin-right: 0.3rem;
		border: none;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-faint);
		font-size: 1rem;
		line-height: 1;
	}
	.mini:hover {
		background: var(--surface-hover);
		color: var(--text);
	}

	.agent-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.1rem;
	}
	.agent-list li {
		position: relative;
	}
	.agent-list li.active .conv {
		background: var(--primary-soft);
		color: var(--text);
		box-shadow: inset 2px 0 0 var(--primary);
	}
	.provider {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		cursor: default;
	}
	.tag {
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
	a.mini {
		text-decoration: none;
	}

	.agent-form {
		display: flex;
		flex-direction: column;
		gap: 0.35rem;
		margin: 0.4rem 0.3rem 0;
	}
	.agent-form input:not([type='checkbox']),
	.agent-form textarea {
		width: 100%;
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-sm);
		color: var(--text);
		padding: 0.4rem 0.55rem;
		font-size: 0.82rem;
		resize: vertical;
	}
	.agent-form .check {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		color: var(--text-muted);
		font-size: 0.8rem;
	}
	.agent-form button[type='submit'] {
		border: none;
		border-radius: var(--radius-sm);
		background: var(--primary);
		color: var(--on-primary);
		padding: 0.45rem;
		font-weight: 600;
	}
	.agent-form button[type='submit']:disabled {
		opacity: 0.45;
		cursor: default;
	}

	.conv-list,
	.file-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.1rem;
	}
	.conv-list li {
		position: relative;
	}
	.conv {
		width: 100%;
		text-align: left;
		background: transparent;
		border: none;
		color: var(--text-muted);
		padding: 0.5rem 2.1rem 0.5rem 0.6rem;
		border-radius: var(--radius-sm);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.conv:hover {
		background: var(--surface-hover);
		color: var(--text);
	}
	.conv-list li.active .conv {
		background: var(--primary-soft);
		color: var(--text);
		box-shadow: inset 2px 0 0 var(--primary);
	}
	.conv-del {
		position: absolute;
		top: 50%;
		right: 0.3rem;
		transform: translateY(-50%);
		display: grid;
		place-items: center;
		width: 28px;
		height: 28px;
		border: none;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-faint);
		opacity: 0;
		transition:
			opacity var(--transition),
			color var(--transition),
			background-color var(--transition);
	}
	.conv-list li:hover .conv-del,
	.agent-list li:hover .conv-del,
	.conv-del:focus-visible {
		opacity: 1;
	}
	/* Agents/providers have no per-row highlight, so keep their delete button
	   discoverable without requiring a hover. */
	.agent-list .conv-del {
		opacity: 0.45;
	}
	.agent-list .conv-del:hover {
		opacity: 1;
	}
	.conv-del:hover {
		color: var(--danger);
		background: var(--danger-bg);
	}
	.conv-del svg {
		width: 15px;
		height: 15px;
	}
	/* The rotate-key button sits just left of the delete button. */
	.conv-del.key {
		right: 2.35rem;
	}
	.conv-del.key:hover {
		color: var(--accent);
		background: var(--primary-soft);
	}

	.file-item {
		display: flex;
		align-items: center;
		gap: 0.45rem;
		padding: 0.4rem 0.6rem;
		border-radius: var(--radius-sm);
		color: var(--text-muted);
		font-size: 0.82rem;
	}
	.file-item:hover {
		background: var(--surface-hover);
		color: var(--text);
	}
	.file-item svg {
		width: 15px;
		height: 15px;
		flex: 0 0 auto;
	}
	.file-name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.utility {
		display: flex;
		align-items: center;
		gap: 0.55rem;
		margin-top: 1.1rem;
		padding: 0.5rem 0.6rem;
		border-radius: var(--radius-sm);
		color: var(--text-muted);
		font-size: 0.85rem;
	}
	.utility:hover {
		background: var(--surface-hover);
		color: var(--text);
	}
	.utility.active {
		background: var(--surface-active);
		color: var(--text);
	}
	.utility svg {
		width: 16px;
		height: 16px;
	}

	.side-foot {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		padding: 0.7rem 0.8rem;
		border-top: 1px solid var(--border);
	}
	.sidebar.collapsed .side-foot {
		justify-content: center;
		padding: 0.7rem 0;
	}
	.avatar {
		display: grid;
		place-items: center;
		width: 36px;
		height: 36px;
		flex: 0 0 auto;
		border: none;
		border-radius: 50%;
		background: linear-gradient(135deg, var(--primary-hover), var(--primary-active));
		color: var(--on-primary);
		font-weight: 700;
		box-shadow: var(--shadow-sm);
	}
	.who-text {
		display: flex;
		flex-direction: column;
		min-width: 0;
		line-height: 1.25;
	}
	.name {
		font-size: 0.85rem;
		color: var(--text);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.email {
		font-size: 0.75rem;
		color: var(--text-muted);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.side-foot .icon-btn {
		margin-left: auto;
	}

	main {
		flex: 1;
		min-width: 0;
		overflow-y: auto;
		background: var(--bg);
	}
	main.bare {
		height: 100vh;
	}
</style>
