<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import * as api from '#lib/api';

	let token = $state<string | null>(null);
	let password = $state('');
	let confirm = $state('');
	let error = $state<string | null>(null);
	let busy = $state(false);
	let done = $state(false);

	onMount(() => {
		token = page.url.searchParams.get('token');
		if (!token) error = 'This link is missing its token.';
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!token) return;
		if (password !== confirm) {
			error = 'The two passwords do not match.';
			return;
		}
		busy = true;
		error = null;
		try {
			await api.confirmPasswordReset(token, password);
			done = true;
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}
</script>

<div class="card">
	<h1>Reset password</h1>
	{#if done}
		<p class="hint">Your password has been changed and every session was signed out.</p>
		<a class="primary" href="/login">Go to sign in</a>
	{:else}
		<form onsubmit={submit}>
			<label>
				New password
				<input type="password" bind:value={password} autocomplete="new-password" minlength="8" required />
			</label>
			<label>
				Repeat password
				<input type="password" bind:value={confirm} autocomplete="new-password" minlength="8" required />
			</label>
			{#if error}<p class="error">{error}</p>{/if}
			<button class="primary" type="submit" disabled={busy || !token}>
				{busy ? 'Please wait…' : 'Set new password'}
			</button>
		</form>
	{/if}
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
	form {
		display: flex;
		flex-direction: column;
		gap: 0.9rem;
	}
	label {
		display: flex;
		flex-direction: column;
		gap: 0.35rem;
		font-size: 0.85rem;
		color: var(--text-muted);
	}
	input {
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius);
		color: var(--text);
		padding: 0.6rem 0.75rem;
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
		margin: 0;
		padding: 0.45rem 0.7rem;
		font-size: 0.85rem;
	}
	.primary {
		display: block;
		text-align: center;
		text-decoration: none;
		margin-top: 0.25rem;
		background: linear-gradient(135deg, var(--primary-hover), var(--primary-active));
		border: none;
		border-radius: var(--radius);
		color: var(--on-primary);
		padding: 0.65rem;
		font-weight: 600;
		box-shadow: var(--shadow-sm);
	}
	.primary:hover:not(:disabled) {
		box-shadow: var(--shadow-glow);
		filter: brightness(1.06);
	}
	.primary:disabled {
		opacity: 0.55;
		cursor: default;
		box-shadow: none;
	}
</style>
