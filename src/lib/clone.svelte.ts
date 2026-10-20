import type { CloneSource } from './types';

// the Clone page's choices and progress, kept while visiting other pages
export const clonePage = $state({
	from: '', // the printer copied from, as typed
	source: null as CloneSource | null, // as last read
	pick: [] as string[], // ids of the settings to copy
	to: '', // the printer copied to, as typed
	useFolder: true,
	target: '' // where Configure lists the printer being copied to, once started
});
