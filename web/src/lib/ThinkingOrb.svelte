<script lang="ts">
	/**
	 * Thin Svelte wrapper around `mountOrb` from `@yogesharc/thinking-orbs`.
	 * The orb draws in `currentColor`, so tint it through CSS `color`.
	 * https://thinkingorbs.com
	 */
	import {
		mountOrb,
		type OrbState,
		type OrbVariant
	} from '@yogesharc/thinking-orbs/vanilla';

	type OrbMountOptions = NonNullable<Parameters<typeof mountOrb>[1]>;

	let {
		state = 'reasoning' as OrbState,
		variant,
		size = 20,
		speed = 1,
		label = 'Thinking',
		class: className = ''
	}: {
		state?: OrbState;
		variant?: OrbVariant;
		size?: number;
		speed?: number;
		label?: string;
		class?: string;
	} = $props();

	let svg: SVGSVGElement | undefined;

	$effect(() => {
		if (!svg) return;
		const options = { state, variant, size, speed, label } as OrbMountOptions;
		const orb = mountOrb(svg, options);
		return () => orb.destroy();
	});
</script>

<svg
	bind:this={svg}
	class={className}
	width={size}
	height={size}
	viewBox={`0 0 ${size} ${size}`}
	role="img"
	aria-label={label}
></svg>

<style>
	svg {
		display: block;
		flex: 0 0 auto;
		color: var(--primary);
	}
</style>
