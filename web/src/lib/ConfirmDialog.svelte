<script lang="ts">
	/**
	 * A small themed confirmation dialog used in place of the browser `confirm`.
	 * Dismisses on Escape or a click on the backdrop.
	 */
	let {
		open = false,
		title = 'Are you sure?',
		message = '',
		confirmLabel = 'Confirm',
		cancelLabel = 'Cancel',
		danger = false,
		onconfirm,
		oncancel
	}: {
		open?: boolean;
		title?: string;
		message?: string;
		confirmLabel?: string;
		cancelLabel?: string;
		danger?: boolean;
		onconfirm?: () => void;
		oncancel?: () => void;
	} = $props();

	$effect(() => {
		if (!open) return;
		const onPointerDown = (event: MouseEvent) => {
			const target = event.target as Element | null;
			if (target?.classList.contains('backdrop')) oncancel?.();
		};
		const onKeydown = (event: KeyboardEvent) => {
			if (event.key === 'Escape') oncancel?.();
		};
		window.addEventListener('mousedown', onPointerDown);
		window.addEventListener('keydown', onKeydown);
		return () => {
			window.removeEventListener('mousedown', onPointerDown);
			window.removeEventListener('keydown', onKeydown);
		};
	});
</script>

{#if open}
	<div class="backdrop">
		<div class="dialog" role="alertdialog" aria-modal="true" aria-label={title}>
			<h2>{title}</h2>
			{#if message}<p>{message}</p>{/if}
			<div class="actions">
				<button class="btn cancel" onclick={oncancel}>{cancelLabel}</button>
				<button class="btn confirm" class:danger onclick={onconfirm}>{confirmLabel}</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.backdrop {
		position: fixed;
		inset: 0;
		z-index: 100;
		display: grid;
		place-items: center;
		padding: 1rem;
		background: color-mix(in srgb, #000 55%, transparent);
		backdrop-filter: blur(3px);
		animation: fade 140ms var(--ease) both;
	}
	.dialog {
		width: min(400px, 100%);
		padding: 1.4rem;
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-lg);
		background: var(--bg-elevated);
		box-shadow: var(--shadow);
		animation: rise 180ms var(--ease) both;
	}
	h2 {
		margin: 0 0 0.5rem;
		font-size: 1.05rem;
		letter-spacing: -0.01em;
	}
	p {
		margin: 0;
		color: var(--text-muted);
		font-size: 0.9rem;
		line-height: 1.5;
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		gap: 0.5rem;
		margin-top: 1.3rem;
	}
	.btn {
		border-radius: var(--radius-full);
		padding: 0.5rem 1.1rem;
		font-weight: 600;
		border: 1px solid transparent;
	}
	.cancel {
		background: transparent;
		border-color: var(--border-strong);
		color: var(--text);
	}
	.cancel:hover {
		background: var(--surface-hover);
	}
	.confirm {
		background: linear-gradient(135deg, var(--primary-hover), var(--primary-active));
		color: var(--on-primary);
		box-shadow: var(--shadow-sm);
	}
	.confirm.danger {
		background: var(--danger);
		color: #fff;
	}
	.confirm:hover {
		filter: brightness(1.06);
	}
	@keyframes fade {
		from {
			opacity: 0;
		}
		to {
			opacity: 1;
		}
	}
</style>
