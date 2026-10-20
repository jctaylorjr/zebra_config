// a short message at the bottom of the window that fades on its own, e.g. after copying
export const toast = $state({ text: '', error: false, id: 0 });

let timer: ReturnType<typeof setTimeout> | undefined;

export function notify(text: string, error = false) {
	toast.text = text;
	toast.error = error;
	toast.id++;
	clearTimeout(timer);
	timer = setTimeout(() => (toast.text = ''), 2200);
}
