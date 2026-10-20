// the rows a drag covers, from where it started to where the pointer is now, applied on top of
// what was selected before. `add` false takes them out instead, for Shift-dragging over rows
// that were already selected
export function dragSelect(
	base: Set<string>,
	rows: string[],
	anchor: number,
	to: number,
	add: boolean
): Set<string> {
	const next = new Set(base);
	for (let i = Math.min(anchor, to); i <= Math.max(anchor, to); i++) {
		if (add) next.add(rows[i]);
		else next.delete(rows[i]);
	}
	return next;
}
