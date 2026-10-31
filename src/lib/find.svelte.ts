import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { needsLookup } from './types';

// where a printer search box's text led: an address as typed, or the printer a control number
// is, with a note on how it was found
export type Found = { address: string; note: string | null };

// what each lookup under way is waiting on, by what was typed, e.g. "Looking up 722858 in
// ServiceNow" or "Sign in to ServiceNow in the browser window"
export const finding = $state<Record<string, string>>({});

let listening: Promise<unknown> | undefined;

// the address for what was typed in a printer search box. a control number is looked up in
// ServiceNow for the printer's name, as is a name made of one that no printer answers to
export async function findPrinter(typed: string): Promise<Found> {
	const query = typed.trim();
	if (!needsLookup(query)) return { address: query, note: null };
	listening ??= listen<[string, string]>('find-status', (e) => {
		const [q, text] = e.payload;
		if (q in finding) finding[q] = text;
	});
	await listening;
	finding[query] = 'Looking it up';
	try {
		return await invoke<Found>('find_printer', { query });
	} finally {
		delete finding[query];
	}
}
