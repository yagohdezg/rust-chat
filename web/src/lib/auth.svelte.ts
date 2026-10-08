import { browser } from '$app/env';

export type User = { id: string; email?: string; name?: string; role: string };
export type Session = { token: string; user: User };

const STORAGE_KEY = 'rust-chat.session';

/**
 * Client-side session store. Persists to `localStorage` so a refresh keeps the
 * user signed in. Runes require this to live in a `.svelte.ts` module.
 */
class AuthStore {
	session = $state<Session | null>(null);
	ready = $state(false);

	constructor() {
		if (browser) {
			const raw = localStorage.getItem(STORAGE_KEY);
			if (raw) {
				try {
					this.session = JSON.parse(raw) as Session;
				} catch {
					localStorage.removeItem(STORAGE_KEY);
				}
			}
			this.ready = true;
		}
	}

	get token(): string | null {
		return this.session?.token ?? null;
	}

	set(session: Session) {
		this.session = session;
		if (browser) localStorage.setItem(STORAGE_KEY, JSON.stringify(session));
	}

	clear() {
		this.session = null;
		if (browser) localStorage.removeItem(STORAGE_KEY);
	}
}

export const auth = new AuthStore();
