/// <reference types="bun-types" />
import { describe, expect, test } from 'bun:test';
import { recentLabel, type Recent } from './types';

const printer = (r: Partial<Recent>): Recent => ({
	address: '192.0.2.5',
	model: null,
	serial: null,
	hostname: null,
	ip_addr: null,
	last_used: 0,
	...r
});

describe('recentLabel', () => {
	test('names a printer by hostname, with the address it was opened by', () => {
		expect(recentLabel(printer({ hostname: 'CSCXYZ123', model: 'ZD621' }))).toBe(
			'CSCXYZ123 (192.0.2.5) · ZD621'
		);
	});

	test("doesn't repeat an address that is the hostname", () => {
		expect(recentLabel(printer({ address: 'cscxyz123', hostname: 'CSCXYZ123' }))).toBe('CSCXYZ123');
	});

	test('falls back to the address', () => {
		expect(recentLabel(printer({}))).toBe('192.0.2.5');
	});
});
