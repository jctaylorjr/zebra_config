import { invoke } from '@tauri-apps/api/core';
import type { Recent } from './types';

// printers opened on the Printer and Diagnose pages, newest first, kept between sessions
export const recent = $state({ list: [] as Recent[], loaded: false });

export async function loadRecent() {
	if (recent.loaded) return;
	recent.list = await invoke<Recent[]>('recent_printers');
	recent.loaded = true;
}

export async function remember(
	address: string,
	info: { model?: string | null; serial?: string | null; hostname?: string | null; ipAddr?: string | null }
) {
	recent.list = await invoke<Recent[]>('remember_printer', {
		address,
		model: info.model ?? null,
		serial: info.serial ?? null,
		hostname: info.hostname ?? null,
		ipAddr: info.ipAddr ?? null
	});
	recent.loaded = true;
}

export async function forget(address: string) {
	recent.list = await invoke<Recent[]>('forget_printer', { address });
}
