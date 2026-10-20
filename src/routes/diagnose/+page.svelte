<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { invoke } from '@tauri-apps/api/core';
	import { confirm } from '@tauri-apps/plugin-dialog';
	import { openUrl } from '@tauri-apps/plugin-opener';
	import { onMount, tick, untrack } from 'svelte';
	import RecentPrinters from '$lib/RecentPrinters.svelte';
	import { copy } from '$lib/clipboard';
	import {
		applyFix,
		diagnosePage,
		diagnosisFor,
		identify,
		loadChecks,
		picked,
		runDiagnosis
	} from '$lib/diagnose.svelte';
	import { recent } from '$lib/recent.svelte';
	import {
		diagnoseHref,
		fits,
		printerHref,
		recentLabel,
		type Finding,
		type LogLine,
		type Printer,
		type Profile
	} from '$lib/types';

	let ip = $derived(page.url.searchParams.get('ip') ?? '');
	// the diagnosis lives in the store, so it's still here, and still running, after visiting
	// other pages
	let d = $derived(ip ? diagnosePage.runs[ip] : undefined);
	let terminal = $state<HTMLElement>();

	onMount(() => {
		(async () => {
			diagnosePage.printers = await invoke<Printer[]>('list_printers');
			diagnosePage.profiles = (await invoke<{ profiles: Profile[] }>('list_profiles')).profiles;
			await loadChecks();
		})();
	});

	// opening a printer remembers it for the sidebar
	$effect.pre(() => {
		if (!ip) return;
		untrack(() => {
			diagnosePage.ip = ip;
			diagnosisFor(ip);
			identify(ip);
		});
	});

	// keep the newest log line in view
	$effect(() => {
		d?.log.length;
		tick().then(() => terminal?.scrollTo({ top: terminal.scrollHeight }));
	});

	let printer = $derived(diagnosePage.printers.find((p) => p.ip === ip));
	// compare with the printer's own profile unless another was chosen
	let profileId = $derived(d?.profileId ?? printer?.profile_id ?? '');
	let fitting = $derived(
		diagnosePage.profiles.filter((p) => !printer || fits(p.models, printer.model))
	);
	let feeding = $derived(diagnosePage.checks.filter((c) => c.feeds && picked.has(c.id)));

	const numbered = (keep: (f: Finding) => boolean) =>
		(d?.findings ?? []).map((f, i) => ({ f, i })).filter(({ f }) => keep(f));
	let problems = $derived(numbered((f) => f.severity === 'error' || f.severity === 'warning'));
	let notes = $derived(numbered((f) => f.severity === 'info'));
	let passed = $derived(numbered((f) => f.severity === 'ok'));
	let fixable = $derived(
		(d?.findings ?? []).map((f, i) => ({ f, i })).filter(({ f, i }) => f.fix && !d?.fixed[i]?.done)
	);

	function open(target: string) {
		if (target.trim()) goto(diagnoseHref(target.trim()));
	}

	function setGroup(feeds: boolean, on: boolean) {
		for (const c of diagnosePage.checks.filter((c) => c.feeds === feeds)) {
			if (on) picked.add(c.id);
			else picked.delete(c.id);
		}
	}

	async function start() {
		if (feeding.length) {
			const ok = await confirm(
				`${feeding.map((c) => c.label).join(', ')} will feed and print labels. Each only runs if media is loaded and the head is closed.`,
				{ title: 'Tests that feed media' }
			);
			if (!ok) return;
		}
		runDiagnosis(ip, profileId);
	}

	const risky = ['restart', 'clear_error'];

	async function fix(i: number, ask = true) {
		const f = d?.findings?.[i];
		if (!f?.fix) return;
		if (ask && risky.includes(f.fix.kind)) {
			const ok = await confirm(
				f.fix.kind === 'restart'
					? 'Restart the printer? It takes about a minute to come back.'
					: 'This cancels all jobs and restarts the printer to clear the error, then pauses it.',
				{ title: f.fix_label ?? 'Fix' }
			);
			if (!ok) return;
		}
		await applyFix(ip, i);
	}

	async function fixAll() {
		const target = ip;
		const list = fixable.filter(({ i }) => !d?.fixed[i]?.busy);
		const ok = await confirm(
			`Apply ${list.length} fixes?\n\n${list.map(({ f }) => `• ${f.fix_label}`).join('\n')}`,
			{ title: 'Apply all fixes' }
		);
		if (!ok) return;
		for (const { i } of list) await applyFix(target, i);
	}

	const icon: Record<string, string> = { error: '✗', warning: '!', info: 'i', ok: '✓' };
	const marks: Record<string, string> = {
		send: '> ',
		reply: '  ',
		step: '── ',
		ok: '✓ ',
		info: 'i ',
		warn: '! ',
		error: '✗ '
	};
	const prompt = (l: LogLine) => marks[l.kind] ?? '';

	function copyLog() {
		copy((d?.log ?? []).map((l) => prompt(l) + l.text).join('\n'), 'the log');
	}

	let name = $derived(d?.identity?.hostname ?? ip);
	let address = $derived(d?.identity?.ip_addr ?? '');
	let recentOthers = $derived(
		recent.list.filter(
			(r) => !diagnosePage.printers.some((p) => p.ip.toLowerCase() === r.address.toLowerCase())
		)
	);
</script>

<div class="top">
	<h1>
		Diagnose{ip ? ` ${name}` : ''}
		{#if ip && address && address !== name}<span class="address">{address}</span>{/if}
	</h1>
	<select value={ip} onchange={(e) => open(e.currentTarget.value)}>
		<option value="" disabled>Choose a printer…</option>
		{#if diagnosePage.printers.length}
			<optgroup label="Configure list">
				{#each diagnosePage.printers as p}
					<option value={p.ip}>{p.ip}{p.model ? ` · ${p.model}` : ''}</option>
				{/each}
			</optgroup>
		{/if}
		{#if recentOthers.length}
			<optgroup label="Recent">
				{#each recentOthers as r}<option value={r.address}>{recentLabel(r)}</option>{/each}
			</optgroup>
		{/if}
	</select>
	<form
		onsubmit={(e) => {
			e.preventDefault();
			open(diagnosePage.typed);
		}}
	>
		<input bind:value={diagnosePage.typed} placeholder="or another IP / hostname" />
		<button type="submit" disabled={!diagnosePage.typed.trim()}>Open</button>
	</form>
	{#if ip}<a class="button" href={printerHref(ip)}>Printer page</a>{/if}
</div>

{#if !ip}
	<p class="muted">Choose a printer to diagnose, or open one from its Printer page.</p>
	<RecentPrinters hrefFor={diagnoseHref} />
{:else if d}
	<section class="card">
		<div class="row">
			<label class="field">
				<span>Compare with profile</span>
				<select value={profileId} onchange={(e) => d && (d.profileId = e.currentTarget.value)}>
					<option value="">None</option>
					{#each fitting as p}<option value={p.id}>{p.name}</option>{/each}
				</select>
			</label>
		</div>
		<div class="groups">
			{#each [false, true] as feeds}
				<div class="group">
					<div class="group-head">
						<strong>{feeds ? 'Tests that feed media' : 'Read the printer'}</strong>
						<button class="link" onclick={() => setGroup(feeds, true)}>All</button>
						<button class="link" onclick={() => setGroup(feeds, false)}>None</button>
					</div>
					{#if feeds}
						<p class="muted small">These print labels, and only run if media is loaded.</p>
					{/if}
					{#each diagnosePage.checks.filter((c) => c.feeds === feeds) as c}
						<label class="check">
							<input
								type="checkbox"
								checked={picked.has(c.id)}
								onchange={(e) => (e.currentTarget.checked ? picked.add(c.id) : picked.delete(c.id))}
							/>
							<span>{c.label}<small>{c.about}</small></span>
						</label>
					{/each}
				</div>
			{/each}
		</div>
		<div class="row">
			<button class="primary" disabled={d.running || picked.size === 0} onclick={start}>
				{d.running ? 'Diagnosing…' : 'Run diagnosis'}
			</button>
			<span class="muted">{picked.size} checks, plus the connection</span>
			{#if d.error}<span class="error">{d.error}</span>{/if}
		</div>
	</section>

	{#if d.log.length}
		<div class="term-head">
			<strong>Log</strong>
			<button class="link" onclick={copyLog}>Copy</button>
		</div>
		<div class="terminal" bind:this={terminal} role="log">
			{#each d.log as l}
				<div class={l.kind}>{prompt(l)}{l.text}</div>
			{/each}
		</div>
	{/if}

	{#if d.findings}
		<section class="card results">
			<div class="row">
				<h2>
					{problems.length === 0
						? 'No problems found'
						: `${problems.length} ${problems.length === 1 ? 'problem' : 'problems'} found`}
				</h2>
				{#if fixable.length > 1}
					<button class="primary" onclick={fixAll}>Apply all {fixable.length} fixes</button>
				{/if}
				<button onclick={start} disabled={d.running}>Run again</button>
			</div>

			{#snippet item(f: Finding, i: number)}
				{@const state = d?.fixed[i]}
				<li class={f.severity}>
					<span class="badge">{icon[f.severity]}</span>
					<div>
						<strong>{f.title}</strong>
						{#if f.detail}<p>{f.detail}</p>{/if}
						{#if f.links.length}
							<p class="links">
								{#each f.links as l}
									<a
										href={l.url}
										onclick={(e) => {
											e.preventDefault();
											openUrl(l.url);
										}}>{l.title} ↗</a
									>
								{/each}
							</p>
						{/if}
						{#if f.fix}
							<p class="fix">
								{#if state?.done}
									<span class="ok">✓ {state.done}</span>
								{:else}
									<button disabled={state?.busy} onclick={() => fix(i)}>
										{state?.busy ? 'Working…' : f.fix_label}
									</button>
									{#if state?.error}<span class="error">{state.error}</span>{/if}
								{/if}
							</p>
						{/if}
					</div>
				</li>
			{/snippet}

			{#if problems.length}
				<ul>
					{#each problems as { f, i }}{@render item(f, i)}{/each}
				</ul>
			{/if}
			{#if notes.length}
				<h3>Notes</h3>
				<ul>
					{#each notes as { f, i }}{@render item(f, i)}{/each}
				</ul>
			{/if}
			{#if passed.length}
				<details>
					<summary>{passed.length} checks passed</summary>
					<ul>
						{#each passed as { f, i }}{@render item(f, i)}{/each}
					</ul>
				</details>
			{/if}
		</section>
	{/if}
{/if}

<style>
	.top,
	.row {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 10px;
		margin-bottom: 12px;
	}
	.top h1 {
		margin: 0 8px 0 0;
	}
	.address {
		font-weight: 400;
		color: var(--muted);
		margin-left: 8px;
	}
	.top form {
		display: flex;
		gap: 6px;
	}
	a.button {
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 5px 12px;
		color: inherit;
		text-decoration: none;
		background: var(--panel);
	}
	.card {
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 12px 16px;
		margin-bottom: 14px;
	}
	.field {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.groups {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
		gap: 8px 24px;
		margin-bottom: 12px;
	}
	.group-head {
		display: flex;
		align-items: baseline;
		gap: 8px;
		margin-bottom: 6px;
	}
	.check {
		display: flex;
		align-items: flex-start;
		gap: 8px;
		padding: 3px 0;
	}
	.check input {
		margin-top: 3px;
	}
	.check small {
		display: block;
		color: var(--muted);
	}
	.small {
		font-size: 12px;
		margin: 0 0 6px;
	}
	button.link {
		border: none;
		background: none;
		padding: 0;
		color: var(--accent);
	}
	.term-head {
		display: flex;
		gap: 10px;
		align-items: baseline;
		margin-bottom: 4px;
	}
	.terminal {
		background: #0f1217;
		color: #c9d1d9;
		font: 12px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace;
		border-radius: 8px;
		padding: 10px 12px;
		height: 280px;
		overflow: auto;
		margin: 0 0 14px;
	}
	.terminal div {
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}
	.terminal .step {
		color: #e6e9ef;
		font-weight: 700;
		margin-top: 4px;
	}
	.terminal .send {
		color: #79c0ff;
	}
	.terminal .reply {
		color: #8b949e;
	}
	.terminal .ok {
		color: #56d364;
	}
	.terminal .info {
		color: #a5d6ff;
	}
	.terminal .warn {
		color: #e3b341;
	}
	.terminal .error {
		color: #ff7b72;
	}
	.results h2 {
		margin: 0;
	}
	.results h3 {
		font-size: 14px;
		margin: 14px 0 6px;
	}
	.results ul {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	.results li {
		display: flex;
		gap: 10px;
		padding: 8px 0;
		border-top: 1px solid var(--border);
	}
	.results li p {
		margin: 4px 0 0;
		white-space: pre-line;
	}
	.badge {
		flex: none;
		width: 22px;
		height: 22px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		font-weight: 700;
		font-size: 12px;
		color: #fff;
		background: var(--muted);
	}
	li.error .badge {
		background: var(--danger);
	}
	li.warning .badge {
		background: var(--warn);
	}
	li.ok .badge {
		background: var(--ok);
	}
	.links {
		display: flex;
		flex-wrap: wrap;
		gap: 4px 14px;
	}
	.links a {
		color: var(--accent);
	}
	.ok {
		color: var(--ok);
	}
	details summary {
		cursor: pointer;
		margin: 12px 0 6px;
		color: var(--muted);
	}
</style>
