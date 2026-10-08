<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import * as api from '#lib/api';
	import { auth } from '#lib/auth.svelte';

	type Tab = 'users' | 'providers' | 'audit';

	let tab = $state<Tab>('users');
	let users = $state<api.AdminUserSummary[]>([]);
	let providers = $state<api.Provider[]>([]);
	let audit = $state<api.AuditLog[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let notice = $state<string | null>(null);

	let pName = $state('OpenAI');
	let pKind = $state('openai');
	let pBaseUrl = $state('https://api.openai.com/v1');
	let pKey = $state('');
	let bulk = $state('');
	let busy = $state(false);

	// New-user form.
	let nuEmail = $state('');
	let nuName = $state('');
	let nuPassword = $state('');
	let nuRole = $state('user');
	let userBusy = $state(false);

	const isAdmin = $derived(auth.session?.user.role === 'admin');
	const selfId = $derived(auth.session?.user.id);
	const globalProviders = $derived(providers.filter((p) => !p.user_id));

	onMount(() => {
		if (!auth.session) {
			goto('/login');
			return;
		}
		if (!isAdmin) {
			goto('/chat');
			return;
		}
		void load();
	});

	async function load() {
		loading = true;
		error = null;
		try {
			[users, providers, audit] = await Promise.all([
				api.listAdminUsers(),
				api.listProviders(),
				api.listAudit(100)
			]);
		} catch (err) {
			error = message(err);
		} finally {
			loading = false;
		}
	}

	async function patchUser(id: string, input: api.UpdateAdminUserInput) {
		error = null;
		notice = null;
		try {
			const updated = await api.updateAdminUser(id, input);
			users = users.map((u) => (u.id === id ? { ...u, ...updated } : u));
			notice = 'Account updated.';
			void refreshAudit();
		} catch (err) {
			error = message(err);
		}
	}

	function toggleRole(user: api.AdminUserSummary) {
		void patchUser(user.id, { role: user.role === 'admin' ? 'user' : 'admin' });
	}

	function toggleDisabled(user: api.AdminUserSummary) {
		void patchUser(user.id, { disabled: !user.disabled });
	}

	async function addUser(event: SubmitEvent) {
		event.preventDefault();
		if (userBusy) return;
		userBusy = true;
		error = null;
		notice = null;
		try {
			const created = await api.createAdminUser({
				email: nuEmail.trim(),
				password: nuPassword,
				name: nuName.trim() || undefined,
				role: nuRole
			});
			users = [
				...users,
				{ ...created, conversation_count: 0, provider_count: 0, agent_count: 0 }
			];
			nuEmail = '';
			nuName = '';
			nuPassword = '';
			nuRole = 'user';
			notice = `Created ${created.email}.`;
			void refreshAudit();
		} catch (err) {
			error = message(err);
		} finally {
			userBusy = false;
		}
	}

	async function removeUser(user: api.AdminUserSummary) {
		if (
			!confirm(
				`Delete ${user.email}? This removes their conversations, agents, providers and files.`
			)
		)
			return;
		error = null;
		notice = null;
		try {
			await api.deleteAdminUser(user.id);
			users = users.filter((u) => u.id !== user.id);
			notice = `Deleted ${user.email}.`;
			void refreshAudit();
		} catch (err) {
			error = message(err);
		}
	}

	async function refreshAudit() {
		try {
			audit = await api.listAudit(100);
		} catch {
			// audit refresh is best-effort
		}
	}

	async function addProvider(event: SubmitEvent) {
		event.preventDefault();
		if (busy) return;
		busy = true;
		error = null;
		notice = null;
		try {
			const created = await api.createProvider({
				name: pName.trim(),
				kind: pKind,
				base_url: pBaseUrl.trim(),
				api_key: pKey.trim() || undefined,
				global: true
			});
			providers = [...providers, created];
			pKey = '';
			notice = `Added global provider "${created.name}".`;
		} catch (err) {
			error = message(err);
		} finally {
			busy = false;
		}
	}

	async function bulkImport(event: SubmitEvent) {
		event.preventDefault();
		if (busy) return;
		let entries: api.CreateProviderInput[];
		try {
			entries = parseBulk(bulk);
		} catch (err) {
			error = message(err);
			return;
		}
		if (entries.length === 0) {
			error = 'Nothing to import.';
			return;
		}
		busy = true;
		error = null;
		notice = null;
		try {
			const created = await api.bulkCreateProviders(entries);
			providers = [...providers, ...created];
			bulk = '';
			notice = `Imported ${created.length} provider(s).`;
		} catch (err) {
			error = message(err);
		} finally {
			busy = false;
		}
	}

	async function rotateKey(provider: api.Provider) {
		const key = prompt(`New API key for "${provider.name}" (leave blank to clear):`);
		if (key === null) return;
		error = null;
		notice = null;
		try {
			const updated = await api.updateProvider(provider.id, { api_key: key.trim() });
			providers = providers.map((p) => (p.id === updated.id ? updated : p));
			notice = `Updated the key for "${updated.name}".`;
		} catch (err) {
			error = message(err);
		}
	}

	async function removeProvider(provider: api.Provider) {
		if (!confirm(`Delete global provider "${provider.name}"?`)) return;
		error = null;
		notice = null;
		try {
			await api.deleteProvider(provider.id);
			providers = providers.filter((p) => p.id !== provider.id);
		} catch (err) {
			error = message(err);
		}
	}

	/** Parse either a JSON array or `name|base_url|kind|api_key` lines. */
	function parseBulk(text: string): api.CreateProviderInput[] {
		const trimmed = text.trim();
		if (!trimmed) return [];
		if (trimmed.startsWith('[')) {
			const parsed = JSON.parse(trimmed) as api.CreateProviderInput[];
			if (!Array.isArray(parsed)) throw new Error('expected a JSON array');
			return parsed.map((entry) => ({ ...entry, global: true }));
		}
		return trimmed
			.split('\n')
			.map((line) => line.trim())
			.filter(Boolean)
			.map((line) => {
				const [name, base_url, kind = 'openai', api_key] = line
					.split('|')
					.map((part) => part.trim());
				if (!name || !base_url) throw new Error(`invalid line: ${line}`);
				return { name, base_url, kind, api_key: api_key || undefined, global: true };
			});
	}

	function message(err: unknown): string {
		return err instanceof Error ? err.message : String(err);
	}

	function fmt(ts?: string | null): string {
		return ts ? new Date(ts).toLocaleString() : '—';
	}
</script>

<section class="admin">
	<header class="head">
		<div>
			<h1>Admin console</h1>
			<p class="lede">Manage accounts, instance-wide providers, and review the audit trail.</p>
		</div>
		<button class="ghost" onclick={() => void load()} disabled={loading}>Refresh</button>
	</header>

	<nav class="tabs">
		<button class:active={tab === 'users'} onclick={() => (tab = 'users')}>Users</button>
		<button class:active={tab === 'providers'} onclick={() => (tab = 'providers')}>
			Providers
		</button>
		<button class:active={tab === 'audit'} onclick={() => (tab = 'audit')}>Audit</button>
	</nav>

	{#if error}<p class="error">{error}</p>{/if}
	{#if notice}<p class="notice">{notice}</p>{/if}

	{#if loading}
		<p class="empty">Loading…</p>
	{:else if tab === 'users'}
		<form class="inline-form" onsubmit={addUser}>
			<input type="email" bind:value={nuEmail} placeholder="email" required />
			<input bind:value={nuName} placeholder="name (optional)" />
			<input type="password" bind:value={nuPassword} placeholder="password" required />
			<select bind:value={nuRole}>
				<option value="user">user</option>
				<option value="admin">admin</option>
			</select>
			<button
				class="primary"
				type="submit"
				disabled={userBusy || !nuEmail.trim() || !nuPassword}
			>
				{userBusy ? 'Adding…' : 'Add user'}
			</button>
		</form>

		<div class="table-wrap">
			<table>
				<thead>
					<tr>
						<th>Account</th>
						<th>Role</th>
						<th>Status</th>
						<th>Last seen</th>
						<th class="num">Chats</th>
						<th class="num">Providers</th>
						<th class="num">Agents</th>
						<th></th>
					</tr>
				</thead>
				<tbody>
					{#each users as user (user.id)}
						<tr class:disabled={user.disabled}>
							<td>
								<span class="email">{user.email}</span>
								{#if user.name}<span class="name">{user.name}</span>{/if}
							</td>
							<td><span class="tag {user.role}">{user.role}</span></td>
							<td>
								<span class="dot" class:on={!user.disabled}></span>
								{user.disabled ? 'Disabled' : 'Active'}
							</td>
							<td>{fmt(user.last_seen_at)}</td>
							<td class="num">{user.conversation_count}</td>
							<td class="num">{user.provider_count}</td>
							<td class="num">{user.agent_count}</td>
							<td>
								{#if user.id !== selfId}
									<div class="actions">
										<button class="mini" onclick={() => toggleRole(user)}>
											{user.role === 'admin' ? 'Demote' : 'Promote'}
										</button>
										<button
											class="mini danger"
											onclick={() => toggleDisabled(user)}
											title={user.disabled ? 'Enable account' : 'Disable account'}
										>
											{user.disabled ? 'Enable' : 'Disable'}
										</button>
										<button
											class="mini danger"
											onclick={() => void removeUser(user)}
											title="Delete account"
										>
											Delete
										</button>
									</div>
								{:else}
									<span class="self">you</span>
								{/if}
							</td>
						</tr>
					{:else}
						<tr><td colspan="8" class="empty">No accounts.</td></tr>
					{/each}
				</tbody>
			</table>
		</div>
	{:else if tab === 'providers'}
		<div class="grid">
			<div class="panel">
				<h2>Instance-wide providers</h2>
				{#if globalProviders.length === 0}
					<p class="empty">No global providers configured.</p>
				{:else}
					<ul class="providers">
						{#each globalProviders as provider (provider.id)}
							<li>
								<div class="prov">
									<span class="pname">{provider.name}</span>
									<span class="purl" title={provider.base_url}>{provider.base_url}</span>
									<span class="tag">{provider.kind}</span>
								</div>
								<div class="actions">
									<button class="mini" onclick={() => void rotateKey(provider)}>Rotate key</button>
									<button class="mini danger" onclick={() => void removeProvider(provider)}>
										Delete
									</button>
								</div>
							</li>
						{/each}
					</ul>
				{/if}
			</div>

			<div class="panel">
				<h2>Add a global provider</h2>
				<form class="stack" onsubmit={addProvider}>
					<label>Label <input bind:value={pName} required /></label>
					<label>
						Kind
						<select bind:value={pKind}>
							<option value="openai">OpenAI</option>
							<option value="custom">OpenAI-compatible</option>
						</select>
					</label>
					<label>Base URL <input bind:value={pBaseUrl} required /></label>
					<label>
						API key
						<input type="password" bind:value={pKey} placeholder="sk-…" autocomplete="off" />
					</label>
					<button class="primary" type="submit" disabled={busy || !pName.trim() || !pBaseUrl.trim()}>
						{busy ? 'Saving…' : 'Add provider'}
					</button>
				</form>

				<h2>Bulk import</h2>
				<form class="stack" onsubmit={bulkImport}>
					<textarea
						bind:value={bulk}
						rows="4"
						placeholder={'JSON array, or one per line:\nname | base_url | kind | api_key'}
					></textarea>
					<button class="ghost" type="submit" disabled={busy || !bulk.trim()}>
						{busy ? 'Importing…' : 'Import'}
					</button>
				</form>
			</div>
		</div>
	{:else}
		<div class="table-wrap">
			<table>
				<thead>
					<tr>
						<th>When</th>
						<th>Action</th>
						<th>Actor</th>
						<th>Target</th>
						<th>IP</th>
						<th>Details</th>
					</tr>
				</thead>
				<tbody>
					{#each audit as entry (entry.id)}
						<tr>
							<td class="nowrap">{fmt(entry.created_at)}</td>
							<td><code>{entry.action}</code></td>
							<td class="mono">{entry.actor_id ? entry.actor_id.slice(0, 8) : '—'}</td>
							<td class="mono">
								{entry.target_type ?? '—'}{entry.target_id
									? `:${entry.target_id.slice(0, 8)}`
									: ''}
							</td>
							<td class="mono">{entry.ip ?? '—'}</td>
							<td class="details">
								{entry.metadata ? JSON.stringify(entry.metadata) : '—'}
							</td>
						</tr>
					{:else}
						<tr><td colspan="6" class="empty">No audit entries yet.</td></tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</section>

<style>
	.admin {
		max-width: 1080px;
		margin: 0 auto;
		padding: 2rem clamp(1rem, 4vw, 2.5rem);
	}
	.head {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 1rem;
	}
	h1 {
		margin: 0 0 0.25rem;
		font-size: 1.5rem;
		letter-spacing: -0.02em;
	}
	.lede {
		margin: 0;
		color: var(--text-muted);
		font-size: 0.88rem;
	}
	.tabs {
		display: flex;
		gap: 0.4rem;
		margin: 1.5rem 0 1rem;
		border-bottom: 1px solid var(--border);
	}
	.tabs button {
		background: none;
		border: none;
		border-bottom: 2px solid transparent;
		color: var(--text-muted);
		padding: 0.55rem 0.9rem;
		font-weight: 600;
	}
	.tabs button:hover {
		color: var(--text);
	}
	.tabs button.active {
		color: var(--text);
		border-bottom-color: var(--primary);
	}

	.table-wrap {
		overflow-x: auto;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--bg-elevated);
	}
	table {
		width: 100%;
		border-collapse: collapse;
		font-size: 0.86rem;
	}
	th,
	td {
		text-align: left;
		padding: 0.65rem 0.8rem;
		border-bottom: 1px solid var(--border);
		vertical-align: middle;
	}
	th {
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-faint);
	}
	tbody tr:last-child td {
		border-bottom: none;
	}
	tbody tr:hover {
		background: var(--surface-hover);
	}
	/* Mute disabled rows without fading the row border (which made the
	   horizontal rules look uneven). */
	tr.disabled td {
		color: var(--text-faint);
	}
	tr.disabled .email {
		color: var(--text-muted);
	}
	tr.disabled .tag {
		opacity: 0.7;
	}
	.num {
		text-align: right;
	}
	.nowrap {
		white-space: nowrap;
	}
	.mono,
	code {
		font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
		font-size: 0.8rem;
		color: var(--text-muted);
	}
	.details {
		max-width: 260px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-faint);
	}
	.email {
		display: block;
		color: var(--text);
	}
	.name {
		display: block;
		color: var(--text-muted);
		font-size: 0.78rem;
	}
	.self {
		display: inline-flex;
		align-items: center;
		min-height: 26px;
		color: var(--text-faint);
		font-size: 0.8rem;
		font-style: italic;
	}

	.tag {
		display: inline-block;
		padding: 0.1rem 0.5rem;
		border-radius: var(--radius-full);
		background: var(--surface-active);
		color: var(--text-muted);
		font-size: 0.68rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}
	.tag.admin {
		background: var(--primary-soft);
		color: var(--accent);
	}
	.dot {
		display: inline-block;
		width: 8px;
		height: 8px;
		margin-right: 0.4rem;
		border-radius: 50%;
		background: var(--danger);
	}
	.dot.on {
		background: #4caf7d;
	}

	.actions {
		display: flex;
		gap: 0.35rem;
		align-items: center;
		justify-content: flex-end;
		flex-wrap: nowrap;
		white-space: nowrap;
		min-height: 26px;
	}
	.mini {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		height: 26px;
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-muted);
		padding: 0 0.55rem;
		font-size: 0.76rem;
		line-height: 1;
		white-space: nowrap;
	}
	.mini:hover {
		background: var(--surface-hover);
		color: var(--text);
	}
	.mini.danger:hover {
		color: var(--danger);
		background: var(--danger-bg);
		border-color: var(--danger);
	}

	.inline-form {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
		margin-bottom: 1rem;
	}
	.inline-form input,
	.inline-form select {
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-sm);
		color: var(--text);
		padding: 0.5rem 0.65rem;
	}
	.inline-form input[type='email'] {
		flex: 1 1 200px;
	}
	.inline-form input:not([type='email']) {
		flex: 1 1 150px;
	}
	.inline-form .primary {
		padding: 0.5rem 1rem;
	}

	.grid {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 1.25rem;
	}
	@media (max-width: 760px) {
		.grid {
			grid-template-columns: 1fr;
		}
	}
	.panel {
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--bg-elevated);
		padding: 1.1rem 1.2rem;
	}
	.panel h2 {
		margin: 0 0 0.8rem;
		font-size: 0.8rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-faint);
	}
	.panel h2:not(:first-child) {
		margin-top: 1.4rem;
	}
	.providers {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}
	.providers li {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 0.75rem;
		padding: 0.55rem 0.7rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
	}
	.prov {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
	.pname {
		font-weight: 600;
	}
	.purl {
		color: var(--text-muted);
		font-size: 0.76rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 260px;
	}
	.stack {
		display: flex;
		flex-direction: column;
		gap: 0.7rem;
	}
	.stack label {
		display: flex;
		flex-direction: column;
		gap: 0.3rem;
		font-size: 0.8rem;
		color: var(--text-muted);
	}
	input,
	select,
	textarea {
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-sm);
		color: var(--text);
		padding: 0.5rem 0.65rem;
		resize: vertical;
	}
	.primary {
		border: none;
		border-radius: var(--radius-sm);
		background: linear-gradient(135deg, var(--primary-hover), var(--primary-active));
		color: var(--on-primary);
		padding: 0.55rem;
		font-weight: 600;
	}
	.primary:disabled {
		opacity: 0.5;
		cursor: default;
	}
	.ghost {
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text);
		padding: 0.5rem 0.85rem;
	}
	.ghost:hover:not(:disabled) {
		background: var(--surface-hover);
	}
	.ghost:disabled {
		opacity: 0.5;
	}
	.empty {
		color: var(--text-faint);
		font-size: 0.86rem;
		text-align: center;
		padding: 1rem;
	}
	.error {
		color: var(--danger);
		background: var(--danger-bg);
		border: 1px solid color-mix(in srgb, var(--danger) 40%, transparent);
		border-radius: var(--radius-sm);
		padding: 0.55rem 0.8rem;
		font-size: 0.86rem;
	}
	.notice {
		color: var(--text-muted);
		font-size: 0.86rem;
	}
</style>
