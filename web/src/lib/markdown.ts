import { browser } from '$app/env';
import DOMPurify from 'dompurify';
import hljs from 'highlight.js/lib/core';
import bash from 'highlight.js/lib/languages/bash';
import c from 'highlight.js/lib/languages/c';
import cpp from 'highlight.js/lib/languages/cpp';
import csharp from 'highlight.js/lib/languages/csharp';
import css from 'highlight.js/lib/languages/css';
import diff from 'highlight.js/lib/languages/diff';
import go from 'highlight.js/lib/languages/go';
import java from 'highlight.js/lib/languages/java';
import javascript from 'highlight.js/lib/languages/javascript';
import json from 'highlight.js/lib/languages/json';
import kotlin from 'highlight.js/lib/languages/kotlin';
import markdown from 'highlight.js/lib/languages/markdown';
import php from 'highlight.js/lib/languages/php';
import plaintext from 'highlight.js/lib/languages/plaintext';
import python from 'highlight.js/lib/languages/python';
import ruby from 'highlight.js/lib/languages/ruby';
import rust from 'highlight.js/lib/languages/rust';
import shell from 'highlight.js/lib/languages/shell';
import sql from 'highlight.js/lib/languages/sql';
import swift from 'highlight.js/lib/languages/swift';
import typescript from 'highlight.js/lib/languages/typescript';
import xml from 'highlight.js/lib/languages/xml';
import yaml from 'highlight.js/lib/languages/yaml';
import { marked } from 'marked';
import { markedHighlight } from 'marked-highlight';

const languages: Record<string, Parameters<typeof hljs.registerLanguage>[1]> = {
	bash,
	c,
	cpp,
	csharp,
	css,
	diff,
	go,
	java,
	javascript,
	json,
	kotlin,
	markdown,
	php,
	plaintext,
	python,
	ruby,
	rust,
	shell,
	sql,
	swift,
	typescript,
	xml,
	yaml
};

for (const [name, language] of Object.entries(languages)) {
	hljs.registerLanguage(name, language);
}

marked.use(
	markedHighlight({
		emptyLangClass: 'hljs',
		langPrefix: 'hljs language-',
		highlight(code, lang) {
			const language = hljs.getLanguage(lang) ? lang : 'plaintext';
			return hljs.highlight(code, { language }).value;
		}
	}),
	{ gfm: true, breaks: true }
);

if (browser) {
	DOMPurify.addHook('afterSanitizeAttributes', (node) => {
		if (node.nodeName === 'A') {
			node.setAttribute('target', '_blank');
			node.setAttribute('rel', 'noopener noreferrer');
		}
	});
}

export function renderMarkdown(content: string | null | undefined): string {
	if (!browser) return '';
	return DOMPurify.sanitize(marked.parse(content ?? '', { async: false }) as string);
}
