import { SvelteSet } from 'svelte/reactivity';

// choices on the Configure page, kept while visiting other pages
export const picks = $state({
	profile: {} as Record<string, string>, // printer ip -> profile id
	useFolder: {} as Record<string, boolean> // printer ip -> send the Zebra folder too
});

export const selected = new SvelteSet<string>(); // printer ips

// a printer that came back at another address after a restart keeps its choices
export function follow(from: string, to: string) {
	if (selected.delete(from)) selected.add(to);
	for (const choices of [picks.profile, picks.useFolder] as Record<string, unknown>[]) {
		if (from in choices) {
			choices[to] = choices[from];
			delete choices[from];
		}
	}
}
