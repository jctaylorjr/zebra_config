// whether the window is focused and showing. the Printer page only polls its printer then;
// work in the backend, like applying profiles or a running diagnosis, carries on regardless
export const focus = $state({ active: true });

if (typeof window !== 'undefined') {
	const showing = () => document.visibilityState === 'visible';
	focus.active = document.hasFocus() && showing();
	window.addEventListener('focus', () => (focus.active = showing()));
	window.addEventListener('blur', () => (focus.active = false));
	document.addEventListener('visibilitychange', () => (focus.active = showing() && document.hasFocus()));
}
