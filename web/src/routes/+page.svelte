<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { auth } from '#lib/auth.svelte';

	onMount(() => {
		goto(auth.session ? '/chat' : '/login');
	});
</script>

<p>Loading…</p>
