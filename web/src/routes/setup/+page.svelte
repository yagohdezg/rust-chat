<script lang="ts">
	import { goto } from '$app/navigation';
	import { auth } from '#lib/auth.svelte';
	import { chat } from '#lib/chat.svelte';

	let name = $state('OpenAI');
	let kind = $state('openai');
	let baseUrl = $state('https://api.openai.com/v1');
	let apiKey = $state('');
	let global = $state(true);
	let busy = $state(false);
	let error = $state<string | null>(null);

	const isAdmin = $derived(auth.session?.user.role === 'admin');

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = null;
		try {
			const provider = await chat.createProvider({
				name: name.trim(),
				kind,
				base_url: baseUrl.trim(),
				api_key: apiKey.trim() || undefined,
				global: isAdmin && global
			});
			if (provider) {
				goto('/chat');
			} else {
				error = chat.error ?? 'Could not save the provider.';
			}
		} finally {
			busy = false;
		}
	}
</script>

<div class="card">
	<h1>Connect a model provider</h1>
	<p class="lede">
		This instance has no model provider yet. Add an OpenAI-compatible endpoint to start chatting.
	</p>
	<form onsubmit={submit}>
		<label>
			Label
			<input bind:value={name} required placeholder="OpenAI" />
		</label>
		<label>
			Kind
			<select bind:value={kind}>
				<option value="openai">OpenAI</option>
				<option value="custom">OpenAI-compatible</option>
			</select>
		</label>
		<label>
			Base URL
			<input bind:value={baseUrl} required placeholder="https://api.openai.com/v1" />
		</label>
		<label>
			API key
			<input type="password" bind:value={apiKey} placeholder="sk-…" autocomplete="off" />
		</label>
		{#if isAdmin}
			<label class="check">
				<input type="checkbox" bind:checked={global} />
				Share with every user (instance default)
			</label>
		{/if}

		{#if error}<p class="error">{error}</p>{/if}

		<button class="primary" type="submit" disabled={busy || !name.trim() || !baseUrl.trim()}>
			{busy ? 'Saving…' : 'Save provider'}
		</button>
	</form>
</div>

<style>
	.card {
		position: relative;
		max-width: 430px;
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
		margin: 0 0 0.4rem;
		font-size: 1.35rem;
		letter-spacing: -0.02em;
	}
	.lede {
		margin: 0 0 1.25rem;
		color: var(--text-muted);
		font-size: 0.88rem;
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
	label.check {
		flex-direction: row;
		align-items: center;
		gap: 0.5rem;
	}
	input,
	select {
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
</style>
