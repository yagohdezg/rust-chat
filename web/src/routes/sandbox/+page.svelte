<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import * as api from '#lib/api';
	import { auth } from '#lib/auth.svelte';

	const languages = ['python', 'javascript', 'typescript', 'bash', 'go', 'rust', 'c', 'cpp'];

	const samples: Record<string, string> = {
		python: 'print("hello from the sandbox")',
		javascript: 'console.log("hello from the sandbox")',
		typescript: 'const msg: string = "hello from the sandbox";\nconsole.log(msg)',
		bash: 'echo "hello from the sandbox"',
		go: 'package main\n\nimport "fmt"\n\nfunc main() {\n\tfmt.Println("hello from the sandbox")\n}',
		rust: 'fn main() {\n    println!("hello from the sandbox");\n}',
		c: '#include <stdio.h>\n\nint main(void) {\n    puts("hello from the sandbox");\n    return 0;\n}',
		cpp: '#include <iostream>\n\nint main() {\n    std::cout << "hello from the sandbox" << std::endl;\n    return 0;\n}'
	};

	let language = $state('python');
	let code = $state(samples.python);
	let result = $state<api.ExecResult | null>(null);
	let error = $state<string | null>(null);
	let busy = $state(false);

	onMount(() => {
		if (!auth.session) goto('/login');
	});

	function pick(next: string) {
		language = next;
		code = samples[next] ?? '';
	}

	async function run() {
		busy = true;
		error = null;
		result = null;
		try {
			result = await api.runSandbox({ language, code });
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}
</script>

<div class="page">
	<h1>Code sandbox</h1>
	<p class="muted">
		Runs untrusted code in an isolated backend. Network access is disabled by default.
	</p>

	<div class="toolbar">
		<select bind:value={language} onchange={() => pick(language)} aria-label="language">
			{#each languages as lang (lang)}
				<option value={lang}>{lang}</option>
			{/each}
		</select>
		<button class="primary" onclick={() => void run()} disabled={busy}>
			{busy ? 'Running…' : 'Run'}
		</button>
	</div>

	<textarea
		bind:value={code}
		spellcheck="false"
		rows="14"
		aria-label="source code"
	></textarea>

	{#if error}
		<p class="error">{error}</p>
	{/if}

	{#if result}
		<div class="result">
			<div class="status">
				exit_code <strong>{result.exit_code}</strong>
				{#if result.timed_out}<span class="badge">timed out</span>{/if}
			</div>
			<h2>stdout</h2>
			<pre>{result.stdout || '(empty)'}</pre>
			<h2>stderr</h2>
			<pre class="stderr">{result.stderr || '(empty)'}</pre>
		</div>
	{/if}
</div>

<style>
	.page {
		max-width: 900px;
		margin: 0 auto;
		padding: 1.5rem 1.25rem;
	}
	h1 {
		margin: 0 0 0.35rem;
		font-size: 1.5rem;
		letter-spacing: -0.02em;
	}
	h2 {
		font-size: 0.75rem;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-faint);
		margin: 0.85rem 0 0.35rem;
	}
	.muted {
		color: var(--text-muted);
		font-size: 0.9rem;
		margin-top: 0;
	}
	.toolbar {
		display: flex;
		gap: 0.5rem;
		margin: 1rem 0 0.75rem;
	}
	select {
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius);
		color: var(--text);
		padding: 0.5rem 0.9rem;
	}
	textarea {
		width: 100%;
		resize: vertical;
		background: var(--bg);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius);
		color: var(--text);
		padding: 0.85rem;
		font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
		font-size: 0.9rem;
		line-height: 1.5;
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
	.primary:hover:not(:disabled) {
		box-shadow: var(--shadow-glow);
		filter: brightness(1.06);
	}
	.primary:disabled {
		opacity: 0.5;
		cursor: default;
		box-shadow: none;
	}
	.error {
		color: var(--danger);
	}
	.result {
		margin-top: 1rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-lg);
		background: var(--bg-elevated);
		padding: 0.85rem 1.1rem 1.1rem;
		animation: rise 220ms var(--ease) both;
	}
	.status {
		color: var(--text-muted);
		font-size: 0.9rem;
	}
	.badge {
		margin-left: 0.5rem;
		background: var(--danger-bg);
		color: var(--danger);
		border: 1px solid color-mix(in srgb, var(--danger) 40%, transparent);
		border-radius: var(--radius-full);
		padding: 0.05rem 0.55rem;
		font-size: 0.75rem;
	}
	pre {
		margin: 0;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		padding: 0.65rem 0.8rem;
		font-size: 0.85rem;
	}
	pre.stderr {
		color: var(--danger);
	}
</style>
