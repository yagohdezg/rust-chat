<script lang="ts">
	import { renderMarkdown } from './markdown';

	let { content = '' }: { content?: string | null } = $props();

	let html = $derived(renderMarkdown(content));
</script>

<div class="markdown">{@html html}</div>

<style>
	.markdown {
		overflow-wrap: anywhere;
	}
	.markdown :global(> :first-child) {
		margin-top: 0;
	}
	.markdown :global(> :last-child) {
		margin-bottom: 0;
	}
	.markdown :global(p) {
		margin: 0 0 0.7em;
	}
	.markdown :global(h1),
	.markdown :global(h2),
	.markdown :global(h3),
	.markdown :global(h4),
	.markdown :global(h5),
	.markdown :global(h6) {
		margin: 1.1em 0 0.5em;
		line-height: 1.3;
		font-weight: 650;
		letter-spacing: -0.01em;
	}
	.markdown :global(h1) {
		font-size: 1.4em;
	}
	.markdown :global(h2) {
		font-size: 1.25em;
	}
	.markdown :global(h3) {
		font-size: 1.1em;
	}
	.markdown :global(h4),
	.markdown :global(h5),
	.markdown :global(h6) {
		font-size: 1em;
	}
	.markdown :global(ul),
	.markdown :global(ol) {
		margin: 0 0 0.7em;
		padding-left: 1.4em;
	}
	.markdown :global(li) {
		margin: 0.2em 0;
	}
	.markdown :global(li > ul),
	.markdown :global(li > ol) {
		margin: 0.2em 0;
	}
	.markdown :global(a) {
		color: var(--accent);
		text-decoration: underline;
		text-underline-offset: 2px;
	}
	.markdown :global(a:hover) {
		color: var(--primary-hover);
	}
	.markdown :global(code) {
		font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
		font-size: 0.86em;
		background: color-mix(in srgb, var(--bg) 70%, black);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		padding: 0.1em 0.35em;
	}
	.markdown :global(pre) {
		margin: 0 0 0.7em;
		padding: 0.7rem 0.85rem;
		background: color-mix(in srgb, var(--bg) 70%, black);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		overflow-x: auto;
	}
	.markdown :global(pre code) {
		background: none;
		border: none;
		padding: 0;
		font-size: 0.82rem;
		line-height: 1.5;
	}
	.markdown :global(blockquote) {
		margin: 0 0 0.7em;
		padding: 0.1em 0.9em;
		border-left: 3px solid var(--border-strong);
		color: var(--text-muted);
	}
	.markdown :global(hr) {
		border: none;
		border-top: 1px solid var(--border);
		margin: 1em 0;
	}
	.markdown :global(table) {
		width: 100%;
		margin: 0 0 0.7em;
		border-collapse: collapse;
		font-size: 0.9em;
	}
	.markdown :global(th),
	.markdown :global(td) {
		border: 1px solid var(--border);
		padding: 0.35rem 0.6rem;
		text-align: left;
	}
	.markdown :global(th) {
		background: var(--surface-active);
	}
	.markdown :global(img) {
		max-width: 100%;
		border-radius: var(--radius-sm);
	}
	.markdown :global(input[type='checkbox']) {
		margin-right: 0.4em;
	}

	.markdown :global(.hljs-comment),
	.markdown :global(.hljs-quote) {
		color: var(--text-faint);
		font-style: italic;
	}
	.markdown :global(.hljs-keyword),
	.markdown :global(.hljs-selector-tag),
	.markdown :global(.hljs-doctag) {
		color: #ff8a65;
	}
	.markdown :global(.hljs-string),
	.markdown :global(.hljs-regexp),
	.markdown :global(.hljs-addition) {
		color: #9cce6a;
	}
	.markdown :global(.hljs-number),
	.markdown :global(.hljs-literal) {
		color: #ffb454;
	}
	.markdown :global(.hljs-built_in),
	.markdown :global(.hljs-type) {
		color: #4dd0c0;
	}
	.markdown :global(.hljs-title),
	.markdown :global(.hljs-section),
	.markdown :global(.hljs-symbol),
	.markdown :global(.hljs-bullet),
	.markdown :global(.hljs-link) {
		color: #f0b45f;
	}
	.markdown :global(.hljs-name),
	.markdown :global(.hljs-tag) {
		color: #ff8a65;
	}
	.markdown :global(.hljs-attr),
	.markdown :global(.hljs-attribute),
	.markdown :global(.hljs-variable),
	.markdown :global(.hljs-template-variable),
	.markdown :global(.hljs-property) {
		color: #e88f66;
	}
	.markdown :global(.hljs-meta) {
		color: #c792ea;
	}
	.markdown :global(.hljs-subst),
	.markdown :global(.hljs-operator),
	.markdown :global(.hljs-punctuation) {
		color: var(--text-muted);
	}
	.markdown :global(.hljs-deletion) {
		color: var(--danger);
	}
	.markdown :global(.hljs-emphasis) {
		font-style: italic;
	}
	.markdown :global(.hljs-strong) {
		font-weight: 600;
	}
</style>
