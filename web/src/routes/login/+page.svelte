<script lang="ts">
	import { goto } from '$app/navigation';
	import * as api from '#lib/api';
	import { auth } from '#lib/auth.svelte';

	let mode = $state<'login' | 'register'>('login');
	let email = $state('');
	let password = $state('');
	let name = $state('');
	let error = $state<string | null>(null);
	let busy = $state(false);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = null;
		try {
			const session =
				mode === 'login'
					? await api.login(email, password)
					: await api.register(email, password, name || undefined);
			auth.set(session);
			goto('/chat');
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}
</script>

<div class="card">
	<h1>{mode === 'login' ? 'Sign in' : 'Create account'}</h1>
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
		<label>
			Password
			<input
				type="password"
				bind:value={password}
				autocomplete={mode === 'login' ? 'current-password' : 'new-password'}
				required
			/>
		</label>

		{#if error}<p class="error">{error}</p>{/if}

		<button class="primary" type="submit" disabled={busy}>
			{busy ? 'Please wait…' : mode === 'login' ? 'Sign in' : 'Register'}
		</button>
	</form>

	<p class="switch">
		{mode === 'login' ? 'No account yet?' : 'Already registered?'}
		<button
			class="link"
			onclick={() => {
				mode = mode === 'login' ? 'register' : 'login';
				error = null;
			}}
		>
			{mode === 'login' ? 'Create one' : 'Sign in'}
		</button>
	</p>
</div>

<style>
	.card {
		max-width: 380px;
		margin: 4rem auto;
		padding: 1.5rem;
		border: 1px solid #1e2430;
		border-radius: 12px;
		background: #0e1118;
	}
	h1 {
		margin: 0 0 1rem;
		font-size: 1.25rem;
	}
	form {
		display: flex;
		flex-direction: column;
		gap: 0.85rem;
	}
	label {
		display: flex;
		flex-direction: column;
		gap: 0.3rem;
		font-size: 0.85rem;
		color: #9aa4b2;
	}
	input {
		background: #0b0d12;
		border: 1px solid #2a3345;
		border-radius: 8px;
		color: #e6e8ee;
		padding: 0.55rem 0.7rem;
		font: inherit;
	}
	input:focus {
		outline: 2px solid #2563eb;
		outline-offset: 1px;
	}
	.primary {
		background: #2563eb;
		border: none;
		border-radius: 8px;
		color: #fff;
		padding: 0.6rem;
		font-weight: 600;
	}
	.primary:disabled {
		opacity: 0.6;
		cursor: default;
	}
	.error {
		color: #fca5a5;
		margin: 0;
		font-size: 0.85rem;
	}
	.switch {
		margin: 1rem 0 0;
		font-size: 0.85rem;
		color: #9aa4b2;
	}
	.link {
		background: none;
		border: none;
		color: #7dd3fc;
		padding: 0;
	}
</style>
