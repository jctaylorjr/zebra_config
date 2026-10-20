import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

export type Theme = 'system' | 'light' | 'dark';

// saved in settings.json; see the Settings page
export const prefs = $state({ theme: 'system' as Theme, dataDir: '' });

// the page's colours follow data-theme on <html>; the window's title bar follows setTheme
function apply(theme: Theme) {
	if (theme === 'system') delete document.documentElement.dataset.theme;
	else document.documentElement.dataset.theme = theme;
	getCurrentWindow()
		.setTheme(theme === 'system' ? null : theme)
		.catch(() => {}); // not every platform can theme its title bar
}

export async function loadPrefs() {
	const p = await invoke<{ theme: Theme; data_dir: string }>('preferences');
	prefs.theme = p.theme;
	prefs.dataDir = p.data_dir;
	apply(p.theme);
}

export async function setTheme(theme: Theme) {
	prefs.theme = theme;
	apply(theme);
	await invoke('set_theme', { theme });
}
