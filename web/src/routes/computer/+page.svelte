<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import * as api from '#lib/api';
	import { auth } from '#lib/auth.svelte';
	import ConfirmDialog from '#lib/ConfirmDialog.svelte';

	type Entry = {
		name: string;
		perms: string;
		size: number;
		modified: string;
		dir: boolean;
		link?: string;
	};

	let computer = $state<api.Computer | null>(null);
	let loading = $state(true);
	let busy = $state(false);
	let error = $state<string | null>(null);

	let path = $state('.');
	let entries = $state<Entry[]>([]);
	let listing = $state(false);
	let listError = $state<string | null>(null);

	let library = $state<api.FileRecord[]>([]);
	let libraryLoading = $state(false);

	let confirmDestroy = $state(false);

	onMount(() => {
		if (!auth.session) {
			goto('/login');
			return;
		}
		void refreshComputer();
		void refreshLibrary();
		void browse('.');
	});

	async function refreshComputer() {
		loading = true;
		error = null;
		try {
			computer = await api.getComputer();
		} catch (err) {
			error = message(err);
		} finally {
			loading = false;
		}
	}

	async function refreshLibrary() {
		libraryLoading = true;
		try {
			library = await api.listUserFiles();
		} catch (err) {
			error = message(err);
		} finally {
			libraryLoading = false;
		}
	}

	async function act(action: () => Promise<unknown>) {
		busy = true;
		error = null;
		try {
			await action();
			computer = await api.getComputer();
		} catch (err) {
			error = message(err);
		} finally {
			busy = false;
		}
	}

	function pause() {
		void act(() => api.pauseComputer());
	}

	function resume() {
		void act(() => api.resumeComputer());
	}

	function destroy() {
		confirmDestroy = false;
		void act(() => api.destroyComputer());
	}

	function shellQuote(value: string): string {
		return `'${value.replace(/'/g, `'\\''`)}'`;
	}

	function joinPath(base: string, name: string): string {
		const parts = `${base}/${name}`.split('/').filter((p) => p && p !== '.');
		return parts.length ? parts.join('/') : '.';
	}

	async function browse(target: string) {
		listing = true;
		listError = null;
		try {
			const result = await api.execComputer({
				language: 'bash',
				code: `ls -la -- ${shellQuote(target)}`
			});
			if (result.exit_code !== 0) {
				listError = result.stderr.trim() || `ls exited with code ${result.exit_code}`;
				entries = [];
			} else {
				entries = parseListing(result.stdout);
				path = target;
			}
		} catch (err) {
			listError = message(err);
			entries = [];
		} finally {
			listing = false;
		}
	}

	function parseListing(out: string): Entry[] {
		const rows: Entry[] = [];
		for (const line of out.split('\n')) {
			if (!line || line.startsWith('total ')) continue;
			const match = line.match(/^(\S+)\s+\d+\s+\S+\s+\S+\s+(\d+)\s+(\S+\s+\S+\s+\S+)\s+(.+)$/);
			if (!match) continue;
			const [, perms, size, modified, rest] = match;
			const [name, link] = rest.split(' -> ');
			if (name === '.' || name === '..') continue;
			rows.push({ perms, size: Number(size), modified, name, dir: perms.startsWith('d'), link });
		}
		return rows;
	}

	function openEntry(entry: Entry) {
		if (entry.dir) void browse(joinPath(path, entry.name));
	}

	async function downloadEntry(entry: Entry) {
		error = null;
		const target = joinPath(path, entry.name);
		try {
			const result = await api.execComputer({
				language: 'bash',
				code: `base64 -w0 -- ${shellQuote(target)}`
			});
			if (result.exit_code !== 0) {
				error = result.stderr.trim() || 'Could not read the file.';
				return;
			}
			if (result.truncated) {
				error = `${entry.name} is too large to download from the browser.`;
				return;
			}
			saveBlob(new Blob([base64ToBytes(result.stdout.trim())]), entry.name);
		} catch (err) {
			error = message(err);
		}
	}

	function base64ToBytes(value: string) {
		const binary = atob(value);
		const bytes = new Uint8Array(new ArrayBuffer(binary.length));
		for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
		return bytes;
	}

	function saveBlob(blob: Blob, filename: string) {
		const url = URL.createObjectURL(blob);
		const link = document.createElement('a');
		link.href = url;
		link.download = filename;
		document.body.appendChild(link);
		link.click();
		link.remove();
		URL.revokeObjectURL(url);
	}

	async function downloadLibraryFile(file: api.FileRecord) {
		error = null;
		try {
			await api.downloadFile(file);
		} catch (err) {
			error = message(err);
		}
	}

	function formatSize(bytes: number): string {
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	}

	function formatTime(value: string): string {
		const date = new Date(value);
		return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
	}

	function message(err: unknown): string {
		return err instanceof Error ? err.message : String(err);
	}

	let crumbs = $derived(path === '.' ? [] : path.split('/'));

	function crumbTo(index: number) {
		return crumbs.slice(0, index + 1).join('/');
	}
</script>

<div class="page">
	<header class="head">
		<div>
			<h1>Computer</h1>
			<p class="muted">A persistent workspace that keeps its files between sessions.</p>
		</div>
		{#if computer}
			<span class="state" class:running={computer.state === 'running'} class:paused={computer.state === 'paused'}>
				{computer.state}
			</span>
		{/if}
	</header>

	{#if error}
		<p class="error">{error}</p>
	{/if}

	<section class="card">
		{#if loading && !computer}
			<p class="muted">Loading…</p>
		{:else if computer}
			<dl class="meta">
				<div><dt>Node</dt><dd>{computer.node}</dd></div>
				<div><dt>Volume</dt><dd>{computer.volume ?? 'none'}</dd></div>
				<div><dt>Last active</dt><dd>{formatTime(computer.last_active_at)}</dd></div>
			</dl>
			<div class="actions">
				<button class="ghost" onclick={() => void refreshComputer()} disabled={busy}>Refresh</button>
				{#if computer.state === 'paused'}
					<button class="primary" onclick={resume} disabled={busy}>Resume</button>
				{:else if computer.state !== 'destroyed'}
					<button class="ghost" onclick={pause} disabled={busy}>Pause</button>
				{/if}
				<button class="danger" onclick={() => (confirmDestroy = true)} disabled={busy}>Destroy</button>
			</div>
		{:else}
			<p class="muted">No computer yet. It is created on first use.</p>
			<button class="primary" onclick={() => void refreshComputer()} disabled={busy}>Create</button>
		{/if}
	</section>

	<section class="card">
		<div class="card-head">
			<h2>Workspace</h2>
			<div class="crumbs">
				<button class="crumb" onclick={() => void browse('.')} disabled={listing}>root</button>
				{#each crumbs as crumb, i (i)}
					<span class="sep">/</span>
					<button class="crumb" onclick={() => void browse(crumbTo(i))} disabled={listing}>{crumb}</button>
				{/each}
			</div>
			<button class="ghost" onclick={() => void browse(path)} disabled={listing}>
				{listing ? 'Loading…' : 'Refresh'}
			</button>
		</div>

		{#if listError}
			<p class="error">{listError}</p>
		{:else if entries.length === 0 && !listing}
			<p class="muted">This directory is empty.</p>
		{:else}
			<ul class="listing">
				{#each entries as entry (entry.name)}
					<li class:dir={entry.dir}>
						<button class="entry" onclick={() => openEntry(entry)} disabled={!entry.dir} title={entry.name}>
							<span class="icon" aria-hidden="true">
								{#if entry.dir}
									<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
								<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
							</svg>
								{:else}
									<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
								<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
								<polyline points="14 2 14 8 20 8" />
							</svg>
								{/if}
							</span>
							<span class="name">{entry.name}</span>
							{#if entry.link}<span class="link">→ {entry.link}</span>{/if}
							<span class="size">{entry.dir ? '' : formatSize(entry.size)}</span>
						</button>
						{#if !entry.dir}
							<button class="icon-btn" onclick={() => void downloadEntry(entry)} title="Download" aria-label="Download {entry.name}">
								<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
									<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
									<polyline points="7 10 12 15 17 10" />
									<line x1="12" y1="15" x2="12" y2="3" />
								</svg>
							</button>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</section>

	<section class="card">
		<div class="card-head">
			<h2>File library</h2>
			<button class="ghost" onclick={() => void refreshLibrary()} disabled={libraryLoading}>Refresh</button>
		</div>
		{#if libraryLoading && library.length === 0}
			<p class="muted">Loading…</p>
		{:else if library.length === 0}
			<p class="muted">No stored files yet.</p>
		{:else}
			<ul class="listing">
				{#each library as file (file.id)}
					<li>
						<span class="entry static">
							<span class="icon" aria-hidden="true">
								<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
								<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
								<polyline points="14 2 14 8 20 8" />
							</svg>
							</span>
							<span class="name">{file.filename}</span>
							<span class="size">{formatSize(file.size_bytes)}</span>
						</span>
						<button class="icon-btn" onclick={() => void downloadLibraryFile(file)} title="Download" aria-label="Download {file.filename}">
							<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
								<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
								<polyline points="7 10 12 15 17 10" />
								<line x1="12" y1="15" x2="12" y2="3" />
							</svg>
						</button>
					</li>
				{/each}
			</ul>
		{/if}
	</section>
</div>

<ConfirmDialog
	open={confirmDestroy}
	title="Destroy computer?"
	message="Its box and durable workspace volume are removed. Files saved to your library are kept."
	confirmLabel="Destroy"
	danger
	onconfirm={destroy}
	oncancel={() => (confirmDestroy = false)}
/>

<style>
	.page {
		max-width: 900px;
		margin: 0 auto;
		padding: 1.5rem 1.25rem;
	}
	.head {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 1rem;
	}
	h1 {
		margin: 0 0 0.35rem;
		font-size: 1.5rem;
		letter-spacing: -0.02em;
	}
	h2 {
		margin: 0;
		font-size: 0.78rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-faint);
	}
	.muted {
		color: var(--text-muted);
		font-size: 0.9rem;
		margin: 0;
	}
	.state {
		padding: 0.2rem 0.7rem;
		border-radius: var(--radius-full);
		border: 1px solid var(--border-strong);
		background: var(--surface-active);
		color: var(--text-muted);
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
	}
	.state.running {
		background: var(--primary-soft);
		border-color: color-mix(in srgb, var(--primary) 45%, var(--border));
		color: var(--text);
	}
	.state.paused {
		background: color-mix(in srgb, var(--accent) 14%, transparent);
		border-color: color-mix(in srgb, var(--accent) 40%, var(--border));
		color: var(--accent);
	}
	.card {
		margin-top: 1.1rem;
		padding: 0.9rem 1.05rem 1.05rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-lg);
		background: var(--bg-elevated);
	}
	.card-head {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		margin-bottom: 0.6rem;
	}
	.card-head h2 {
		flex: 0 0 auto;
	}
	.card-head .ghost {
		margin-left: auto;
	}
	.crumbs {
		display: flex;
		align-items: center;
		gap: 0.2rem;
		min-width: 0;
		overflow: hidden;
	}
	.crumb {
		border: none;
		background: transparent;
		color: var(--accent);
		padding: 0.1rem 0.25rem;
		border-radius: var(--radius-sm);
		font-size: 0.82rem;
	}
	.crumb:hover {
		background: var(--surface-hover);
	}
	.sep {
		color: var(--text-faint);
	}
	.meta {
		display: flex;
		flex-wrap: wrap;
		gap: 1.2rem;
		margin: 0 0 0.85rem;
	}
	.meta div {
		display: flex;
		flex-direction: column;
		gap: 0.1rem;
	}
	dt {
		font-size: 0.66rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-faint);
	}
	dd {
		margin: 0;
		font-size: 0.88rem;
	}
	.actions {
		display: flex;
		gap: 0.5rem;
	}
	.primary {
		background: linear-gradient(135deg, var(--primary-hover), var(--primary-active));
		border: none;
		border-radius: var(--radius);
		color: var(--on-primary);
		padding: 0.5rem 1.3rem;
		font-weight: 600;
		box-shadow: var(--shadow-sm);
	}
	.primary:disabled {
		opacity: 0.5;
		cursor: default;
	}
	.ghost {
		background: transparent;
		border: 1px solid var(--border-strong);
		border-radius: var(--radius);
		color: var(--text-muted);
		padding: 0.5rem 1.1rem;
	}
	.ghost:hover:not(:disabled) {
		background: var(--surface-hover);
		color: var(--text);
	}
	.ghost:disabled {
		opacity: 0.5;
		cursor: default;
	}
	.danger {
		background: transparent;
		border: 1px solid color-mix(in srgb, var(--danger) 45%, var(--border));
		border-radius: var(--radius);
		color: var(--danger);
		padding: 0.5rem 1.1rem;
	}
	.danger:hover:not(:disabled) {
		background: var(--danger-bg);
	}
	.listing {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.1rem;
	}
	.listing li {
		display: flex;
		align-items: center;
		gap: 0.2rem;
		border-radius: var(--radius-sm);
	}
	.listing li:hover {
		background: var(--surface-hover);
	}
	.entry {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: center;
		gap: 0.55rem;
		text-align: left;
		border: none;
		background: transparent;
		color: var(--text-muted);
		padding: 0.45rem 0.55rem;
	}
	.entry:disabled {
		cursor: default;
	}
	.entry:not(:disabled):hover,
	.listing li:hover .entry {
		color: var(--text);
	}
	.icon {
		flex: 0 0 auto;
		display: grid;
		place-items: center;
		color: var(--text-faint);
	}
	.icon svg {
		width: 16px;
		height: 16px;
	}
	.name {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.link {
		color: var(--text-faint);
		font-size: 0.78rem;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.size {
		flex: 0 0 auto;
		color: var(--text-faint);
		font-size: 0.76rem;
	}
	.icon-btn {
		display: grid;
		place-items: center;
		width: 30px;
		height: 30px;
		flex: 0 0 auto;
		border: none;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-faint);
	}
	.icon-btn:hover {
		background: var(--surface-hover);
		color: var(--text);
	}
	.icon-btn svg {
		width: 15px;
		height: 15px;
	}
	.error {
		color: var(--danger);
		background: var(--danger-bg);
		border: 1px solid color-mix(in srgb, var(--danger) 40%, transparent);
		border-radius: var(--radius-sm);
		margin: 0.6rem 0 0;
		padding: 0.45rem 0.7rem;
		font-size: 0.85rem;
	}
</style>
