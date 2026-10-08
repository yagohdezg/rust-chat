<script lang="ts">
	import '../app.css';
	import favicon from '#lib/assets/favicon.svg';
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import * as api from '#lib/api';
	import { auth } from '#lib/auth.svelte';
	import { chat } from '#lib/chat.svelte';
	import ConfirmDialog from '#lib/ConfirmDialog.svelte';
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

	/** Per-conversation overflow menu (pin / rename / duplicate / delete). */
	let menuOpenId = $state<string | null>(null);

	function toggleMenu(id: string) {
		menuOpenId = menuOpenId === id ? null : id;
	}

	// Inline rename: the row swaps its title for an input.
	let renamingId = $state<string | null>(null);
	let renameValue = $state('');

	function autofocus(node: HTMLInputElement) {
		node.focus();
		node.select();
	}

	function beginRename(conversation: { id: string; title: string }) {
		menuOpenId = null;
		renamingId = conversation.id;
		renameValue = conversation.title;
	}

	function cancelRename() {
		renamingId = null;
	}

	async function commitRename() {
		const id = renamingId;
		if (!id) return;
		renamingId = null;
		await chat.rename(id, renameValue);
	}

	function onRenameKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter') {
			event.preventDefault();
			void commitRename();
		} else if (event.key === 'Escape') {
			event.preventDefault();
			cancelRename();
		}
	}

	function duplicateConversation(id: string) {
		menuOpenId = null;
		openChat();
		void chat.duplicate(id);
	}

	function togglePin(conversation: { id: string; pinned: boolean }) {
		menuOpenId = null;
		void chat.setPinned(conversation.id, !conversation.pinned);
	}

	// Delete goes through a themed dialog instead of the browser `confirm`.
	let pendingDelete = $state<{ id: string; title: string } | null>(null);

	function requestRemove(id: string, title: string) {
		menuOpenId = null;
		pendingDelete = { id, title };
	}

	function confirmDelete() {
		const target = pendingDelete;
		pendingDelete = null;
		if (target) void chat.remove(target.id);
	}

	// Dismiss the overflow menu on an outside click or Escape.
	$effect(() => {
		if (!menuOpenId) return;
		const onPointerDown = (event: MouseEvent) => {
			const target = event.target as Element | null;
			if (target?.closest('.conv-menu')) return;
			menuOpenId = null;
		};
		const onKeydown = (event: KeyboardEvent) => {
			if (event.key === 'Escape') menuOpenId = null;
		};
		window.addEventListener('mousedown', onPointerDown);
		window.addEventListener('keydown', onKeydown);
		return () => {
			window.removeEventListener('mousedown', onPointerDown);
			window.removeEventListener('keydown', onKeydown);
		};
	});

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
										{#if renamingId === conversation.id}
											<input
												class="conv-rename"
												bind:value={renameValue}
												use:autofocus
												onkeydown={onRenameKeydown}
												onblur={commitRename}
											/>
										{:else}
											<button
												class="conv"
												onclick={() => selectConversation(conversation.id)}
												title={conversation.title}
											>
												{#if conversation.pinned}
													<svg class="pin" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
														<path d="M14 2l8 8-4 1-3 3-1 5-7-7 5-1 3-3z" />
													</svg>
												{/if}
												{conversation.title}
											</button>
											<div class="conv-menu">
												<button
													class="conv-menu-btn"
													class:open={menuOpenId === conversation.id}
													onclick={() => toggleMenu(conversation.id)}
													title="Chat options"
													aria-label="Chat options"
												>
													<svg viewBox="0 0 24 24" fill="currentColor">
														<circle cx="12" cy="5" r="1.6" />
														<circle cx="12" cy="12" r="1.6" />
														<circle cx="12" cy="19" r="1.6" />
													</svg>
												</button>
												{#if menuOpenId === conversation.id}
													<div class="menu-pop">
														<button onclick={() => togglePin(conversation)}>
															{conversation.pinned ? 'Unpin' : 'Pin'}
														</button>
														<button onclick={() => beginRename(conversation)}>Rename</button>
														<button onclick={() => duplicateConversation(conversation.id)}>
															Duplicate
														</button>
														<button
															class="danger"
															onclick={() => requestRemove(conversation.id, conversation.title)}
														>
															Delete
														</button>
													</div>
												{/if}
											</div>
										{/if}
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

	<ConfirmDialog
		open={!!pendingDelete}
		title="Delete chat?"
		message={pendingDelete ? `"${pendingDelete.title}" and its messages will be removed. This cannot be undone.` : ''}
		confirmLabel="Delete"
		danger
		onconfirm={confirmDelete}
		oncancel={() => (pendingDelete = null)}
	/>
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
	.conv .pin {
		width: 12px;
		height: 12px;
		margin-right: 0.35rem;
		color: var(--accent);
		vertical-align: -1px;
	}
	.conv-rename {
		width: calc(100% - 0.6rem);
		margin: 0 0.3rem;
		background: var(--bg);
		border: 1px solid var(--primary);
		border-radius: var(--radius-sm);
		color: var(--text);
		padding: 0.45rem 0.55rem;
		font: inherit;
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

	/* per-conversation overflow menu (rename / delete) */
	.conv-menu {
		position: absolute;
		top: 50%;
		right: 0.3rem;
		transform: translateY(-50%);
	}
	.conv-menu-btn {
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
	.conv-list li:hover .conv-menu-btn,
	.conv-menu-btn:focus-visible,
	.conv-menu-btn.open {
		opacity: 1;
	}
	.conv-menu-btn:hover,
	.conv-menu-btn.open {
		color: var(--text);
		background: var(--surface-hover);
	}
	.conv-menu-btn svg {
		width: 15px;
		height: 15px;
	}
	.menu-pop {
		position: absolute;
		top: calc(100% + 0.2rem);
		right: 0;
		z-index: 30;
		min-width: 140px;
		padding: 0.3rem;
		display: flex;
		flex-direction: column;
		gap: 0.1rem;
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-sm);
		background: var(--bg-elevated);
		box-shadow: var(--shadow);
		animation: rise 140ms var(--ease) both;
	}
	.menu-pop button {
		text-align: left;
		border: none;
		background: transparent;
		color: var(--text-muted);
		padding: 0.45rem 0.55rem;
		border-radius: var(--radius-sm);
		font-size: 0.85rem;
	}
	.menu-pop button:hover {
		background: var(--surface-hover);
		color: var(--text);
	}
	.menu-pop button.danger:hover {
		background: var(--danger-bg);
		color: var(--danger);
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
