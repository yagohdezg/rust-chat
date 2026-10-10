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
import katex from 'katex';
import 'katex/dist/katex.min.css';
import { marked, type Tokens } from 'marked';
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

function convertTexDelimiters(content: string): string {
	let result = '';
	let index = 0;
	while (index < content.length) {
		const atLineStart = index === 0 || content[index - 1] === '\n';
		if (atLineStart) {
			const fence = /^ {0,3}(`{3,}|~{3,})/.exec(content.slice(index));
			if (fence) {
				const marker = fence[1][0];
				const openEnd = content.indexOf('\n', index);
				if (openEnd === -1) {
					result += content.slice(index);
					break;
				}
				const close = new RegExp('^ {0,3}' + marker + '{' + fence[1].length + ',}[ \\t]*$', 'm').exec(
					content.slice(openEnd + 1)
				);
				if (!close) {
					result += content.slice(index);
					break;
				}
				const end = openEnd + 1 + close.index + close[0].length;
				result += content.slice(index, end);
				index = end;
				continue;
			}
		}
		const char = content[index];
		if (char === '`') {
			let ticks = 0;
			while (content[index + ticks] === '`') ticks++;
			const delimiter = '`'.repeat(ticks);
			const close = content.indexOf(delimiter, index + ticks);
			if (close === -1) {
				result += content.slice(index);
				break;
			}
			result += content.slice(index, close + ticks);
			index = close + ticks;
			continue;
		}
		if (char === '\\') {
			const next = content[index + 1];
			if (next === '\\') {
				result += content.slice(index, index + 2);
				index += 2;
				continue;
			}
			if (next === '(' || next === '[') {
				const closeToken = next === '(' ? '\\)' : '\\]';
				const close = content.indexOf(closeToken, index + 2);
				if (close !== -1) {
					const delimiter = next === '(' ? '$' : '$$';
					const inner = content.slice(index + 2, close).replace(/\s*\n\s*/g, ' ');
					result += delimiter + inner + delimiter;
					index = close + 2;
					continue;
				}
			}
		}
		result += char;
		index++;
	}
	return result;
}

const inlineKatexRule =
	/^(\${1,2})(?!\$)((?:\\.|[^\\\n])*?(?:\\.|[^\\\n$]))\1(?=[\s?!.,:;)\]}"'%？！。，：]|$)/;
const blockKatexRule = /^(\${1,2})\n((?:\\[^]|[^\\])+?)\n\1(?:\n|$)/;

function renderKatex(tex: string, displayMode: boolean): string {
	return katex.renderToString(tex, { throwOnError: false, displayMode });
}

const inlineKatex = {
	name: 'inlineKatex',
	level: 'inline' as const,
	start(src: string) {
		let index = src.indexOf('$');
		while (index !== -1) {
			if (inlineKatexRule.test(src.slice(index))) return index;
			index = src.indexOf('$', index + 1);
		}
	},
	tokenizer(src: string) {
		const match = inlineKatexRule.exec(src);
		if (match) {
			return {
				type: 'inlineKatex',
				raw: match[0],
				text: match[2].trim(),
				displayMode: match[1].length === 2
			};
		}
	},
	renderer(token: Tokens.Generic) {
		return renderKatex(String(token.text), Boolean(token.displayMode));
	}
};

const blockKatex = {
	name: 'blockKatex',
	level: 'block' as const,
	tokenizer(src: string) {
		const match = blockKatexRule.exec(src);
		if (match) {
			return {
				type: 'blockKatex',
				raw: match[0],
				text: match[2].trim(),
				displayMode: match[1].length === 2
			};
		}
	},
	renderer(token: Tokens.Generic) {
		return renderKatex(String(token.text), Boolean(token.displayMode)) + '\n';
	}
};

marked.use(
	markedHighlight({
		emptyLangClass: 'hljs',
		langPrefix: 'hljs language-',
		highlight(code, lang) {
			const language = hljs.getLanguage(lang) ? lang : 'plaintext';
			return hljs.highlight(code, { language }).value;
		}
	}),
	{ extensions: [inlineKatex, blockKatex] },
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
	return DOMPurify.sanitize(marked.parse(convertTexDelimiters(content ?? ''), { async: false }) as string, {
		ADD_TAGS: ['semantics', 'annotation'],
		ADD_ATTR: ['encoding']
	});
}
