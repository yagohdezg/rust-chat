<script lang="ts">
	import favicon from '#lib/assets/favicon.svg';
	import { auth } from '#lib/auth.svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();

	function logout() {
		auth.clear();
		goto('/login');
	}
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<title>rust-chat</title>
</svelte:head>

<header>
	<a class="brand" href="/chat">rust-chat</a>
	<nav>
		<a href="/chat" class:active={page.url.pathname.startsWith('/chat')}>Chat</a>
		<a href="/sandbox" class:active={page.url.pathname.startsWith('/sandbox')}>Sandbox</a>
	</nav>
	<div class="who">
		{#if auth.session}
			<span class="email">{auth.session.user.email}</span>
			<button class="ghost" onclick={logout}>Sign out</button>
		{:else}
			<a class="ghost" href="/login">Sign in</a>
		{/if}
	</div>
</header>

<main>
	{@render children()}
</main>

<style>
	:global(*) {
		box-sizing: border-box;
	}
	:global(html, body) {
		margin: 0;
		height: 100%;
	}
	:global(body) {
		background: #0b0d12;
		color: #e6e8ee;
		font: 15px/1.5 ui-sans-serif, system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif;
	}
	:global(a) {
		color: #7dd3fc;
		text-decoration: none;
	}
	:global(code, pre) {
		font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
	}
	:global(button) {
		font: inherit;
		cursor: pointer;
	}

	header {
		display: flex;
		align-items: center;
		gap: 1.25rem;
		padding: 0.65rem 1.25rem;
		border-bottom: 1px solid #1e2430;
		background: #0e1118;
		position: sticky;
		top: 0;
		z-index: 10;
	}
	.brand {
		font-weight: 700;
		color: #e6e8ee;
		letter-spacing: -0.02em;
	}
	nav {
		display: flex;
		gap: 0.5rem;
	}
	nav a {
		color: #9aa4b2;
		padding: 0.25rem 0.6rem;
		border-radius: 6px;
	}
	nav a.active,
	nav a:hover {
		color: #e6e8ee;
		background: #1a2130;
	}
	.who {
		margin-left: auto;
		display: flex;
		align-items: center;
		gap: 0.75rem;
	}
	.email {
		color: #9aa4b2;
		font-size: 0.85rem;
	}
	.ghost {
		background: transparent;
		border: 1px solid #2a3345;
		color: #cfd6e4;
		padding: 0.3rem 0.7rem;
		border-radius: 6px;
	}
	.ghost:hover {
		background: #1a2130;
	}
	main {
		max-width: 1100px;
		margin: 0 auto;
		padding: 1.25rem;
	}
</style>
