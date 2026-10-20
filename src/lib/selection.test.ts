/// <reference types="bun-types" />
import { describe, expect, test } from 'bun:test';
import { dragSelect } from './selection';

const rows = ['a', 'b', 'c', 'd', 'e'];
const sorted = (s: Set<string>) => [...s].sort();

describe('dragSelect', () => {
	test('a plain click selects just that row', () => {
		expect(sorted(dragSelect(new Set(), rows, 2, 2, true))).toEqual(['c']);
	});

	test('dragging down or up selects every row in between', () => {
		expect(sorted(dragSelect(new Set(), rows, 1, 3, true))).toEqual(['b', 'c', 'd']);
		expect(sorted(dragSelect(new Set(), rows, 3, 1, true))).toEqual(['b', 'c', 'd']);
	});

	test('dragging back toward the start shrinks the selection', () => {
		const wide = dragSelect(new Set(), rows, 0, 4, true);
		expect(wide.size).toBe(5);
		expect(sorted(dragSelect(new Set(), rows, 0, 1, true))).toEqual(['a', 'b']);
	});

	test('shift-click adds a row and keeps the rest', () => {
		expect(sorted(dragSelect(new Set(['a']), rows, 3, 3, true))).toEqual(['a', 'd']);
	});

	test('shift-click on a selected row removes only that row', () => {
		expect(sorted(dragSelect(new Set(['a', 'c', 'd']), rows, 2, 2, false))).toEqual(['a', 'd']);
	});

	test('shift-dragging adds a second run to the first', () => {
		expect(sorted(dragSelect(new Set(['a', 'b']), rows, 3, 4, true))).toEqual(['a', 'b', 'd', 'e']);
	});
});
