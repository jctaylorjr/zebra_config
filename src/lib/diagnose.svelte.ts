import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { SvelteSet } from 'svelte/reactivity';
import { remember } from './recent.svelte';
import type { CheckInfo, Finding, Identity, LogLine, Printer, Profile } from './types';

// one printer's diagnosis
export type Diagnosis = {
	identity: Identity | null; // read when the printer is opened, for the title
	profileId: string | null; // null until chosen: the printer's own profile
	running: boolean;
	run: number; // tags the latest run's log lines
	log: LogLine[];
	findings: Finding[] | null;
	fixed: Record<number, { busy?: boolean; done?: string; error?: string }>;
	error: string;
};

// the Diagnose page, kept while visiting other pages. a run carries on, and keeps logging,
// while the page isn't open
export const diagnosePage = $state({
	ip: '', // the printer diagnosed last; the sidebar link goes back to it
	typed: '',
	checks: [] as CheckInfo[],
	printers: [] as Printer[],
	profiles: [] as Profile[],
	runs: {} as Record<string, Diagnosis>
});

export const picked = new SvelteSet<string>(); // check ids, the same for every printer

let lastRun = 0;
const order = { error: 0, warning: 1, info: 2, ok: 3 };

// listened to for as long as the app runs, so a log is complete whichever page is open
listen<[number, LogLine]>('diagnose-log', (e) => {
	const [run, line] = e.payload;
	Object.values(diagnosePage.runs)
		.find((d) => d.run === run)
		?.log.push(line);
});

// the read-only checks start picked; the ones that feed media don't
export async function loadChecks() {
	if (diagnosePage.checks.length) return;
	diagnosePage.checks = await invoke<CheckInfo[]>('diagnose_checks');
	diagnosePage.checks.filter((c) => !c.feeds).forEach((c) => picked.add(c.id));
}

// a printer's diagnosis, made the first time it's opened; for effects and event handlers
export function diagnosisFor(ip: string): Diagnosis {
	diagnosePage.runs[ip] ??= {
		identity: null,
		profileId: null,
		running: false,
		run: 0,
		log: [],
		findings: null,
		fixed: {},
		error: ''
	};
	return diagnosePage.runs[ip];
}

// reads who the printer is for the title, and adds it to the recent printers once it answers
export async function identify(ip: string) {
	const d = diagnosisFor(ip);
	try {
		const who = await invoke<Identity>('identify', { ip });
		d.identity = who;
		remember(ip, { ...who, ipAddr: who.ip_addr });
	} catch {
		// unreachable for now; the diagnosis says so when it runs
	}
}

export async function runDiagnosis(ip: string, profileId: string) {
	const d = diagnosisFor(ip);
	const run = ++lastRun;
	Object.assign(d, { run, running: true, log: [], findings: null, fixed: {}, error: '' });
	try {
		const result = await invoke<Finding[]>('run_diagnostics', {
			ip,
			checks: diagnosePage.checks.filter((c) => picked.has(c.id)).map((c) => c.id),
			profileId: profileId || null,
			pcOffsetMinutes: -new Date().getTimezoneOffset(),
			run
		});
		if (d.run === run) d.findings = result.sort((a, b) => order[a.severity] - order[b.severity]);
	} catch (e) {
		if (d.run === run) d.error = String(e);
	}
	if (d.run === run) d.running = false;
}

export async function applyFix(ip: string, i: number) {
	const d = diagnosisFor(ip);
	const f = d.findings?.[i];
	if (!f?.fix) return;
	d.fixed[i] = { busy: true };
	d.log.push({ kind: 'step', text: `Fix: ${f.fix_label}` });
	try {
		const done = await invoke<string>('apply_fix', { ip, fix: f.fix });
		d.fixed[i] = { done };
		d.log.push({ kind: 'ok', text: done });
	} catch (e) {
		d.fixed[i] = { error: String(e) };
		d.log.push({ kind: 'error', text: String(e) });
	}
}
