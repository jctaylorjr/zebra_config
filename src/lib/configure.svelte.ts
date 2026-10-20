import { SvelteSet } from 'svelte/reactivity';

// choices on the Configure page, kept while visiting other pages
export const picks = $state({
	profile: {} as Record<string, string>, // printer ip -> profile id
	useFolder: {} as Record<string, boolean> // printer ip -> send the Zebra folder too
});

export const selected = new SvelteSet<string>(); // printer ips
