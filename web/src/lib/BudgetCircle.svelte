<script lang="ts">
	let {
		remaining = 0,
		total = 0,
		currency = 'USD',
		tokensChat = 0,
		tokensUser = 0,
		source
	}: {
		remaining?: number;
		total?: number;
		currency?: string;
		tokensChat?: number;
		tokensUser?: number;
		source?: string | null;
	} = $props();

	const RADIUS = 16;
	const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

	let ratio = $derived(total > 0 ? Math.max(0, Math.min(1, remaining / total)) : 0);
	let percent = $derived(Math.round(ratio * 100));
	let dash = $derived(
		`${(ratio * CIRCUMFERENCE).toFixed(2)} ${CIRCUMFERENCE.toFixed(2)}`
	);
	let level = $derived(ratio > 0.25 ? 'ok' : ratio > 0.1 ? 'low' : 'critical');

	function money(value: number): string {
		try {
			return new Intl.NumberFormat(undefined, {
				style: 'currency',
				currency,
				maximumFractionDigits: 2
			}).format(value);
		} catch {
			return `${value.toFixed(2)} ${currency}`;
		}
	}

	function tokens(value: number): string {
		if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
		if (value >= 1_000) return `${(value / 1_000).toFixed(1)}k`;
		return `${value}`;
	}
</script>

<div class="budget">
	<button
		type="button"
		class="ring {level}"
		aria-label="Budget: {percent}% remaining"
	>
		<svg viewBox="0 0 38 38" aria-hidden="true">
			<circle class="track" cx="19" cy="19" r={RADIUS} />
			<circle
				class="value"
				cx="19"
				cy="19"
				r={RADIUS}
				stroke-dasharray={dash}
				transform="rotate(-90 19 19)"
			/>
		</svg>
		<span class="label">{percent}%</span>
	</button>

	<div class="pop" role="tooltip">
		<div class="pop-head">
			<span>Budget</span>
			{#if source}<span class="source">{source}</span>{/if}
		</div>
		<div class="amount">
			<strong>{money(remaining)}</strong>
			<span class="of">of {money(total)} left</span>
		</div>
		<div class="bar"><span class={level} style="width: {percent}%"></span></div>
		<dl class="stats">
			<div><dt>This chat</dt><dd>{tokens(tokensChat)}</dd></div>
			<div><dt>Your account</dt><dd>{tokens(tokensUser)}</dd></div>
		</dl>
		<p class="foot">Estimated — provider balance where available.</p>
	</div>
</div>

<style>
	.budget {
		position: relative;
		display: inline-flex;
	}
	.ring {
		position: relative;
		display: grid;
		place-items: center;
		width: 38px;
		height: 38px;
		flex: 0 0 auto;
		padding: 0;
		border: none;
		background: transparent;
		color: var(--primary);
	}
	.ring.low {
		color: var(--accent);
	}
	.ring.critical {
		color: var(--danger);
	}
	.ring svg {
		width: 38px;
		height: 38px;
	}
	.ring circle {
		fill: none;
		stroke-width: 3;
	}
	.track {
		stroke: var(--border-strong);
	}
	.value {
		stroke: currentColor;
		stroke-linecap: round;
		transition: stroke-dasharray var(--transition), stroke var(--transition);
	}
	.label {
		position: absolute;
		font-size: 0.6rem;
		font-weight: 700;
		letter-spacing: -0.02em;
		color: var(--text);
	}
	.pop {
		position: absolute;
		right: 0;
		bottom: calc(100% + 0.6rem);
		z-index: 20;
		width: 230px;
		padding: 0.7rem 0.8rem;
		border: 1px solid var(--border-strong);
		border-radius: var(--radius);
		background: var(--bg-elevated);
		box-shadow: var(--shadow);
		opacity: 0;
		visibility: hidden;
		transform: translateY(4px);
		transition:
			opacity var(--transition),
			transform var(--transition),
			visibility var(--transition);
	}
	.budget:hover .pop,
	.budget:focus-within .pop {
		opacity: 1;
		visibility: visible;
		transform: translateY(0);
	}
	.pop-head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 0.5rem;
		font-size: 0.68rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-faint);
	}
	.source {
		text-transform: none;
		letter-spacing: 0;
		font-weight: 500;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.amount {
		display: flex;
		align-items: baseline;
		gap: 0.4rem;
		margin: 0.35rem 0 0.5rem;
	}
	.amount strong {
		font-size: 1.15rem;
		font-weight: 650;
		letter-spacing: -0.02em;
	}
	.of {
		color: var(--text-muted);
		font-size: 0.78rem;
	}
	.bar {
		height: 6px;
		border-radius: var(--radius-full);
		background: var(--surface-active);
		overflow: hidden;
	}
	.bar span {
		display: block;
		height: 100%;
		border-radius: inherit;
		background: var(--primary);
		transition: width var(--transition);
	}
	.bar span.low {
		background: var(--accent);
	}
	.bar span.critical {
		background: var(--danger);
	}
	.stats {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		margin: 0.6rem 0 0;
	}
	.stats div {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}
	.stats dt {
		color: var(--text-muted);
		font-size: 0.8rem;
	}
	.stats dd {
		margin: 0;
		font-size: 0.82rem;
		font-variant-numeric: tabular-nums;
	}
	.foot {
		margin: 0.55rem 0 0;
		color: var(--text-faint);
		font-size: 0.68rem;
	}
</style>
