import { notify } from './toast.svelte';

// copies text and says so. if the webview refuses the clipboard API, a hidden text box and
// the older copy command do the job
export async function copy(text: string, what: string) {
	try {
		await navigator.clipboard.writeText(text);
	} catch {
		const box = document.createElement('textarea');
		box.value = text;
		box.style.position = 'fixed';
		box.style.opacity = '0';
		document.body.append(box);
		box.select();
		const ok = document.execCommand('copy');
		box.remove();
		if (!ok) {
			notify(`Couldn't copy ${what}`, true);
			return;
		}
	}
	notify(`Copied ${what}`);
}
