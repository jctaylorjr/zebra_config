// the layout turns shortcut keys into app:refresh, app:find, app:save and app:run events for
// whichever page is open; the Settings page lists the keys
export type Shortcut = 'refresh' | 'find' | 'save' | 'run';

export function onShortcut(name: Shortcut, handler: () => void) {
	const listener = () => handler();
	window.addEventListener(`app:${name}`, listener);
	return () => window.removeEventListener(`app:${name}`, listener);
}

export function fire(name: Shortcut) {
	window.dispatchEvent(new CustomEvent(`app:${name}`));
}
