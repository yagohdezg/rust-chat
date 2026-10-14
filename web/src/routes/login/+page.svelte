<script lang="ts">
	import { goto } from '$app/navigation';
	import * as api from '#lib/api';
	import { auth } from '#lib/auth.svelte';

	type Mode = 'login' | 'register' | 'forgot';
	let mode = $state<Mode>('login');
	let email = $state('');
	let password = $state('');
	let name = $state('');
	let error = $state<string | null>(null);
	let notice = $state<string | null>(null);
	let busy = $state(false);
	let unverified = $state(false);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = null;
		notice = null;
		unverified = false;
		try {
			if (mode === 'forgot') {
				await api.requestPasswordReset(email);
				notice = 'If that address has an account, a reset link is on its way.';
				return;
			}
			const session =
				mode === 'login'
					? await api.login(email, password)
					: await api.register(email, password, name || undefined);
			if (session.user.email_verified === false) {
				mode = 'login';
				notice = 'Account created. Open the link we emailed you to confirm your address.';
				return;
			}
			auth.set(session);
			goto('/chat');
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
			unverified = err instanceof api.ApiError && err.status === 403 && mode === 'login';
		} finally {
			busy = false;
		}
	}

	async function resend() {
		error = null;
		unverified = false;
		await api.resendVerification(email);
		notice = 'A new verification link is on its way.';
	}

	function switchTo(next: Mode) {
		mode = next;
		error = null;
		notice = null;
		unverified = false;
	}
</script>

<div class="card">
	<h1>
		{mode === 'login' ? 'Sign in' : mode === 'register' ? 'Create account' : 'Reset password'}
	</h1>
	<form onsubmit={submit}>
		{#if mode === 'register'}
			<label>
				Name
				<input bind:value={name} autocomplete="name" placeholder="Ada Lovelace" />
			</label>
		{/if}
		<label>
			Email
			<input type="email" bind:value={email} autocomplete="email" required />
		</label>
		{#if mode !== 'forgot'}
			<label>
				Password
				<input
					type="password"
					bind:value={password}
					autocomplete={mode === 'login' ? 'current-password' : 'new-password'}
					minlength="8"
					required
				/>
			</label>
		{/if}

		{#if error}<p class="error">{error}</p>{/if}
		{#if notice}<p class="notice">{notice}</p>{/if}
		{#if unverified}
			<button class="link" type="button" onclick={resend}>Send a new verification link</button>
		{/if}

		<button class="primary" type="submit" disabled={busy}>
			{busy
				? 'Please wait…'
				: mode === 'login'
					? 'Sign in'
					: mode === 'register'
						? 'Register'
						: 'Send reset link'}
		</button>
	</form>

	{#if mode === 'login'}
		<p class="switch">
			Forgot your password?
			<button class="link" onclick={() => switchTo('forgot')}>Reset it</button>
		</p>
	{/if}
	<p class="switch">
		{mode === 'register' ? 'Already registered?' : 'No account yet?'}
		<button class="link" onclick={() => switchTo(mode === 'register' ? 'login' : 'register')}>
			{mode === 'register' ? 'Sign in' : 'Create one'}
		</button>
	</p>
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
	input::placeholder {
		color: var(--text-faint);
	}
	.primary {
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
	.error {
		color: var(--danger);
		background: var(--danger-bg);
		border-radius: var(--radius-sm);
		margin: 0;
		padding: 0.45rem 0.7rem;
		font-size: 0.85rem;
	}
	.notice {
		color: var(--text-muted);
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		margin: 0;
		padding: 0.45rem 0.7rem;
		font-size: 0.85rem;
	}
	.switch {
		margin: 1.25rem 0 0;
		font-size: 0.85rem;
		color: var(--text-muted);
	}
	.link {
		background: none;
		border: none;
		color: var(--accent);
		padding: 0;
		font-weight: 600;
	}
	.link:hover {
		color: var(--primary-hover);
	}
</style>
