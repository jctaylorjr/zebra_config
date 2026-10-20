<script lang="ts">
	import { page } from '$app/state';
	import { diagnosePage } from '$lib/diagnose.svelte';
	import { printerPage } from '$lib/printer.svelte';
	import { diagnoseHref, printerHref } from '$lib/types';

	let { children } = $props();

	// icons from Lucide (ISC) and Feather (MIT); the label printer is drawn for this app
	const sections = [
		{
			href: '/',
			label: 'Configure',
			// clipboard with a check: set printers up from a profile
			icon: '<rect width="8" height="4" x="8" y="2" rx="1" ry="1"/><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/><path d="m9 14 2 2 4-4"/>'
		},
		{
			href: '/printer',
			label: 'Printer',
			// a desktop label printer with a label coming out of its slot
			icon: '<path d="M4 15a2 2 0 0 1-2-2v-3a5 5 0 0 1 5-5h10a5 5 0 0 1 5 5v3a2 2 0 0 1-2 2z"/><path d="M7 15v7h10v-7"/><path d="M9.5 17.5v2"/><path d="M12 17.5v2"/><path d="M14.5 17.5v2"/><path d="M16.5 9h1"/>'
		},
		{
			href: '/diagnose',
			label: 'Diagnose',
			// stethoscope
			icon: '<path d="M11 2v2"/><path d="M5 2v2"/><path d="M5 3H4a2 2 0 0 0-2 2v4a6 6 0 0 0 12 0V5a2 2 0 0 0-2-2h-1"/><path d="M8 15a6 6 0 0 0 12 0v-3"/><circle cx="20" cy="10" r="2"/>'
		},
		{
			href: '/profiles',
			label: 'Profiles',
			// sliders
			icon: '<line x1="4" y1="21" x2="4" y2="14"/><line x1="4" y1="10" x2="4" y2="3"/><line x1="12" y1="21" x2="12" y2="12"/><line x1="12" y1="8" x2="12" y2="3"/><line x1="20" y1="21" x2="20" y2="16"/><line x1="20" y1="12" x2="20" y2="3"/><line x1="1" y1="14" x2="7" y2="14"/><line x1="9" y1="8" x2="15" y2="8"/><line x1="17" y1="16" x2="23" y2="16"/>'
		},
		{
			href: '/settings',
			label: 'Settings',
			// gear
			icon: '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>'
		}
	];

	// the Printer and Diagnose links go back to the printer each had open
	const linkFor = (href: string) =>
		href === '/printer' && printerPage.ip
			? printerHref(printerPage.ip)
			: href === '/diagnose' && diagnosePage.ip
				? diagnoseHref(diagnosePage.ip)
				: href;

	// mouse back and forward buttons, and Alt+arrows, move through the pages visited. the
	// webview's own handling is stopped, so a click doesn't navigate twice
	function mouseNav(e: MouseEvent) {
		if (e.button !== 3 && e.button !== 4) return;
		e.preventDefault();
		if (e.type === 'mouseup') e.button === 3 ? history.back() : history.forward();
	}
</script>

<svelte:window
	onmousedown={mouseNav}
	onmouseup={mouseNav}
	onkeydown={(e) => {
		if (!e.altKey) return;
		if (e.key === 'ArrowLeft') history.back();
		if (e.key === 'ArrowRight') history.forward();
	}}
/>

<div class="app">
	<nav>
		{#each sections as s}
			<a
				href={linkFor(s.href)}
				class:active={s.href === '/'
					? page.url.pathname === '/'
					: page.url.pathname.startsWith(s.href)}
				title={s.label}
			>
				<svg viewBox="0 0 24 24" aria-hidden="true">{@html s.icon}</svg>
				<span>{s.label}</span>
			</a>
		{/each}
	</nav>
	<main>
		{@render children()}
	</main>
</div>

<style>
	:global(:root) {
		--bg: #f6f7f9;
		--panel: #ffffff;
		--fg: #1d2330;
		--muted: #667085;
		--border: #d9dee7;
		--accent: #2f6fde;
		--accent-fg: #ffffff;
		--danger: #c62f3e;
		--warn: #a86400;
		--ok: #1f8a4c;
		color-scheme: light;
	}
	@media (prefers-color-scheme: dark) {
		:global(:root) {
			--bg: #14171c;
			--panel: #1c2027;
			--fg: #e6e9ef;
			--muted: #98a2b3;
			--border: #333a45;
			--accent: #5b8ff0;
			--accent-fg: #0d1117;
			--danger: #ef6b77;
			--warn: #e6a23c;
			--ok: #4cc283;
			color-scheme: dark;
		}
	}
	:global(body) {
		margin: 0;
		background: var(--bg);
		color: var(--fg);
		font: 14px/1.45 system-ui, sans-serif;
	}
	:global(h1) {
		font-size: 20px;
		margin: 0 0 16px;
	}
	:global(h2) {
		font-size: 15px;
		margin: 20px 0 8px;
	}
	:global(input:not([type='checkbox'])),
	:global(select),
	:global(textarea) {
		font: inherit;
		color: inherit;
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 5px 8px;
	}
	:global(button) {
		font: inherit;
		color: inherit;
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 5px 12px;
		cursor: pointer;
	}
	:global(button.primary) {
		background: var(--accent);
		border-color: var(--accent);
		color: var(--accent-fg);
	}
	:global(button:disabled) {
		opacity: 0.5;
		cursor: default;
	}
	:global(table) {
		border-collapse: collapse;
		width: 100%;
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 8px;
	}
	:global(th),
	:global(td) {
		text-align: left;
		padding: 7px 10px;
		border-bottom: 1px solid var(--border);
		vertical-align: top;
	}
	:global(th) {
		color: var(--muted);
		font-weight: 600;
		font-size: 12px;
	}
	:global(code) {
		font-size: 12px;
	}
	:global(.error) {
		color: var(--danger);
	}
	:global(.muted) {
		color: var(--muted);
	}

	.app {
		display: flex;
		min-height: 100vh;
	}
	nav {
		width: 76px;
		flex: none;
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 10px 6px;
		background: var(--panel);
		border-right: 1px solid var(--border);
	}
	nav a {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 3px;
		padding: 8px 0;
		border-radius: 8px;
		color: var(--muted);
		text-decoration: none;
		font-size: 11px;
	}
	nav a:hover {
		color: var(--fg);
	}
	nav a.active {
		color: var(--accent);
		background: color-mix(in srgb, var(--accent) 12%, transparent);
	}
	nav svg {
		width: 22px;
		height: 22px;
		fill: none;
		stroke: currentColor;
		stroke-width: 2;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
	main {
		flex: 1;
		min-width: 0;
		padding: 20px 24px;
	}
</style>
