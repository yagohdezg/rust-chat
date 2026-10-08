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

<style>
	h1 {
		margin: 0 0 0.25rem;
		font-size: 1.35rem;
	}
	h2 {
		font-size: 0.8rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: #7b8794;
		margin: 0.75rem 0 0.3rem;
	}
	.muted {
		color: #7b8794;
		font-size: 0.9rem;
		margin-top: 0;
	}
	.toolbar {
		display: flex;
		gap: 0.5rem;
		margin: 1rem 0 0.75rem;
	}
	select {
		background: #0b0d12;
		border: 1px solid #2a3345;
		border-radius: 8px;
		color: #e6e8ee;
		padding: 0.45rem 0.7rem;
		font: inherit;
	}
	textarea {
		width: 100%;
		resize: vertical;
		background: #0b0d12;
		border: 1px solid #2a3345;
		border-radius: 10px;
		color: #e6e8ee;
		padding: 0.75rem;
		font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
		font-size: 0.9rem;
		line-height: 1.45;
	}
	textarea:focus {
		outline: 2px solid #2563eb;
		outline-offset: 1px;
	}
	.primary {
		background: #2563eb;
		border: none;
		border-radius: 8px;
		color: #fff;
		padding: 0.45rem 1.1rem;
		font-weight: 600;
	}
	.primary:disabled {
		opacity: 0.5;
		cursor: default;
	}
	.error {
		color: #fca5a5;
	}
	.result {
		margin-top: 1rem;
		border: 1px solid #1e2430;
		border-radius: 10px;
		background: #0e1118;
		padding: 0.75rem 1rem 1rem;
	}
	.status {
		color: #9aa4b2;
		font-size: 0.9rem;
	}
	.badge {
		margin-left: 0.5rem;
		background: #7f1d1d;
		color: #fecaca;
		border-radius: 999px;
		padding: 0.05rem 0.5rem;
		font-size: 0.75rem;
	}
	pre {
		margin: 0;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
		background: #0b0d12;
		border: 1px solid #1e2430;
		border-radius: 8px;
		padding: 0.6rem 0.75rem;
		font-size: 0.85rem;
	}
	pre.stderr {
		color: #fca5a5;
	}
</style>
