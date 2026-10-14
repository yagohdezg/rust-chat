<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import * as api from '#lib/api';

	let status = $state<'working' | 'done' | 'failed'>('working');
	let error = $state<string | null>(null);

	onMount(async () => {
		const token = page.url.searchParams.get('token');
		if (!token) {
			status = 'failed';
			error = 'This link is missing its token.';
			return;
		}
		try {
			await api.verifyEmail(token);
			status = 'done';
		} catch (err) {
			status = 'failed';
			error = err instanceof Error ? err.message : String(err);
		}
	});
</script>

<div class="card">
	<h1>Email verification</h1>
	{#if status === 'working'}
		<p class="hint">Confirming your address…</p>
	{:else if status === 'done'}
		<p class="hint">Your address is confirmed. You can sign in now.</p>
	{:else}
		<p class="error">{error}</p>
		<p class="hint">Sign in and ask for a new link if this one has expired.</p>
	{/if}
	<a class="primary" href="/login">Go to sign in</a>
</div>

<style>
	.card {
		position: relative;
		max-width: 390px;
		margin: 4rem auto;
		padding: 1.75rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-lg);
		background: var(--bg-elevated);
		box-shadow: var(--shadow);
		overflow: hidden;
		animation: rise 300ms var(--ease) both;
	}
	.card::before {
		content: '';
		position: absolute;
		inset: 0 0 auto;
		height: 3px;
		background: linear-gradient(90deg, var(--primary-active), var(--primary-hover), var(--accent));
	}
	h1 {
		margin: 0 0 1.25rem;
		font-size: 1.35rem;
		letter-spacing: -0.02em;
	}
	.hint {
		margin: 0 0 1rem;
		font-size: 0.85rem;
		color: var(--text-muted);
	}
	.error {
		color: var(--danger);
		background: var(--danger-bg);
		border-radius: var(--radius-sm);
		margin: 0 0 1rem;
		padding: 0.45rem 0.7rem;
		font-size: 0.85rem;
	}
	.primary {
		display: block;
		text-align: center;
		text-decoration: none;
		background: linear-gradient(135deg, var(--primary-hover), var(--primary-active));
		border-radius: var(--radius);
		color: var(--on-primary);
		padding: 0.65rem;
		font-weight: 600;
		box-shadow: var(--shadow-sm);
	}
	.primary:hover {
		box-shadow: var(--shadow-glow);
		filter: brightness(1.06);
	}
</style>
