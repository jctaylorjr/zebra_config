<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import { confirm } from '@tauri-apps/plugin-dialog';
	import { onMount, untrack } from 'svelte';
	import { picks } from '$lib/configure.svelte';
	import { printerPage, viewFor } from '$lib/printer.svelte';
	import {
		diagnoseHref,
		fits,
		folderFor,
		printerHref,
		statusText,
		type Details,
		type FolderView,
		type Printer,
		type PrinterSetting,
		type Profile
	} from '$lib/types';

	let ip = $derived(page.url.searchParams.get('ip') ?? '');
	// everything shown for this printer lives in the store, so it's still here after visiting
	// other pages
	let v = $derived(ip ? printerPage.views[ip] : undefined);

	// the settings worth seeing first; the full list is further down
	const groups: { title: string; rows: [string, string][] }[] = [
		{
			title: 'Identity',
			rows: [
				['Model', 'device.product_name'],
				['Serial', 'device.unique_id'],
				['Firmware', 'appl.name'],
				['Link-OS', 'appl.link_os_version'],
				['Hostname (7.6 and later)', 'ip.hostname'],
				['Friendly name', 'device.friendly_name'],
				['Up for', 'device.uptime']
			]
		},
		{
			title: 'Network',
			rows: [
				['IP address', 'interface.network.active.ip_addr'],
				['Connected by', 'interface.network.active.cable_type'],
				['Address from', 'interface.network.active.protocol'],
				['MAC address', 'interface.network.active.mac_addr'],
				['Preferred network', 'ip.primary_network'],
				['Wired protocol', 'internal_wired.ip.protocol'],
				['Wireless protocol', 'wlan.ip.protocol'],
				['Wi-Fi network', 'wlan.essid'],
				['Wi-Fi signal', 'wlan.signal_strength']
			]
		},
		{
			title: 'Media and printing',
			rows: [
				['Media type', 'ezpl.media_type'],
				['Sense mode', 'media.sense_mode'],
				['Sensor', 'device.sensor_select'],
				['Print width (dots)', 'ezpl.print_width'],
				['Label length (dots)', 'zpl.label_length'],
				['Resolution (dpi)', 'head.resolution.in_dpi'],
				['Darkness', 'print.tone'],
				['Speed (in/s)', 'media.speed'],
				['Print mode', 'media.printmode']
			]
		},
		{
			title: 'Counters',
			rows: [
				['Labels printed', 'odometer.total_label_count'],
				['Printed since head cleaned', 'odometer.headclean'],
				['Printed since new head', 'odometer.headnew'],
				['Printed in total', 'odometer.total_print_length'],
				['Head opened', 'odometer.latch_open_count']
			]
		}
	];
	const keys = groups.flatMap((g) => g.rows.map((r) => r[1]));

	// the odometer reports distances in centimetres
	const distances = ['odometer.headclean', 'odometer.headnew', 'odometer.total_print_length'];

	const show = (key: string, val: unknown) => {
		if (val === null || val === undefined)
			return key === 'ip.hostname' ? 'not on this firmware' : '—';
		const s = String(val);
		if (distances.includes(key) && /^\d+$/.test(s)) {
			const m = Number(s) / 100;
			return `${Math.round(m).toLocaleString()} m (${Math.round(m * 3.28084).toLocaleString()} ft)`;
		}
		if (key === 'ip.primary_network') return s === '1' ? 'Wired' : s === '2' ? 'Wireless' : s;
		if (key === 'wlan.signal_strength' && /^\d+$/.test(s)) return `${s}%`;
		return s || '—';
	};

	// feeding commands check for media first; the rest just go
	const actions: { id: string; label: string; feeds?: boolean; ask?: string; hint?: string }[] = [
		{ id: 'feed', label: 'Feed a label', feeds: true },
		{ id: 'test_label', label: 'Print test label', feeds: true },
		{ id: 'config_label', label: 'Print configuration', feeds: true },
		{ id: 'network_label', label: 'Print network configuration', feeds: true },
		{
			id: 'sensor_profile',
			label: 'Print sensor profile',
			feeds: true,
			hint: 'Graphs what the media sensor sees across a few labels, for media that won’t calibrate'
		},
		{ id: 'pause', label: 'Pause' },
		{ id: 'resume', label: 'Resume' },
		{ id: 'cancel_jobs', label: 'Cancel all jobs', hint: 'Throws away anything waiting to print' },
		{
			id: 'clear_error',
			label: 'Clear error and pause',
			ask: 'This cancels all jobs and restarts the printer to clear its error, then pauses it. It takes about a minute.'
		},
		{ id: 'restart', label: 'Restart', ask: 'Restart the printer? It takes about a minute to come back.' }
	];

	onMount(() => {
		(async () => {
			printerPage.printers = await invoke<Printer[]>('list_printers');
			printerPage.profiles = (await invoke<{ profiles: Profile[] }>('list_profiles')).profiles;
			printerPage.folders = await invoke<FolderView[]>('list_folders');
		})();
		const un = listen<Printer>('printer-update', (e) => {
			printerPage.printers = printerPage.printers.map((x) =>
				x.ip === e.payload.ip ? e.payload : x
			);
		});
		return () => {
			un.then((f) => f());
		};
	});

	// opening a printer remembers it for the sidebar, reads it, and reads every setting the
	// first time it's opened
	$effect.pre(() => {
		if (!ip) return;
		untrack(() => {
			printerPage.ip = ip;
			const first = !printerPage.views[ip];
			viewFor(ip);
			refresh(ip);
			if (first) loadAll(ip);
		});
	});

	$effect(() => {
		if (!ip || !printerPage.autoRefresh) return;
		const target = ip;
		const timer = setInterval(() => refresh(target), 5000);
		return () => clearInterval(timer);
	});

	let listed = $derived(printerPage.printers.find((p) => p.ip === ip));
	let fitting = $derived(
		printerPage.profiles.filter((p) => fits(p.models, listed?.model ?? null))
	);
	let folder = $derived(listed ? folderFor(printerPage.folders, listed.model) : undefined);
	let chosenProfile = $derived(listed ? (picks.profile[ip] ?? listed.profile_id ?? '') : '');

	// each of these writes to the view of the printer it started on, even if another printer
	// is open by the time it finishes
	async function refresh(target: string) {
		const view = viewFor(target);
		try {
			view.details = await invoke<Details>('printer_details', { ip: target, keys });
			view.detailsError = '';
			view.updated = new Date().toLocaleTimeString();
		} catch (e) {
			view.detailsError = String(e);
		}
	}

	async function loadAll(target: string) {
		const view = viewFor(target);
		view.allLoading = true;
		view.allError = '';
		try {
			view.all = await invoke<PrinterSetting[]>('printer_settings', { ip: target });
		} catch (e) {
			view.allError = String(e);
		}
		view.allLoading = false;
	}

	async function addToList() {
		const view = viewFor(ip);
		view.setupError = '';
		try {
			const p = await invoke<Printer>('add_printer', { ip });
			printerPage.printers = [...printerPage.printers, p];
		} catch (e) {
			view.setupError = String(e);
		}
	}

	async function applyProfile() {
		const view = viewFor(ip);
		view.setupError = '';
		try {
			await invoke('apply_profile', {
				ip,
				profileId: chosenProfile,
				useFolder: picks.useFolder[ip] ?? true
			});
		} catch (e) {
			view.setupError = String(e);
		}
	}

	// the calibration method that matches what the printer is set to now
	const methodFor: Record<string, string> = {
		'gap/notch': 'gap',
		mark: 'mark',
		continuous: 'continuous',
		auto_detect: 'auto'
	};
	let method = $derived(
		v?.calMethod || methodFor[String(v?.details?.values['ezpl.media_type'] ?? '')] || 'gap'
	);

	// feeding with no media or the head open leaves the printer in an error
	async function okToFeed(title: string, doing: string) {
		const d = v?.details;
		if (!d || (d.media && d.head_closed)) return true;
		const why = d.head_closed ? 'doesn’t see any media' : 'has its head open';
		return confirm(`The printer ${why}. ${doing} now would leave it in an error. Go ahead?`, {
			title
		});
	}

	async function calibrate() {
		if (!(await okToFeed('Calibrate', 'Calibrating'))) return;
		const target = ip;
		const view = viewFor(target);
		view.calBusy = true;
		view.calNote = view.calError = '';
		try {
			view.calNote = await invoke<string>('printer_calibrate', { ip: target, method });
		} catch (e) {
			view.calError = String(e);
		}
		view.calBusy = false;
		refresh(target);
	}

	async function run(a: (typeof actions)[number]) {
		if (a.feeds && !(await okToFeed(a.label, 'Feeding'))) return;
		if (a.ask && !(await confirm(a.ask, { title: a.label }))) return;
		const target = ip;
		const view = viewFor(target);
		view.busy = a.id;
		view.actionNote = view.actionError = '';
		try {
			const done = await invoke<string>('printer_action', { ip: target, action: a.id });
			view.actionNote = `${a.label}: ${done}`;
		} catch (e) {
			view.actionError = `${a.label}: ${e}`;
		}
		view.busy = null;
		refresh(target);
	}

	async function sendRaw() {
		const target = ip;
		const view = viewFor(target);
		if (!view.rawCommand.trim()) return;
		view.rawBusy = true;
		try {
			const reply = await invoke<string>('printer_raw', { ip: target, command: view.rawCommand });
			view.rawReply = reply || '(no reply)';
		} catch (e) {
			view.rawReply = `Failed: ${e}`;
		}
		view.rawBusy = false;
	}

	let alerts = $derived.by(() => {
		const d = v?.details;
		if (!d) return [];
		const list = [
			...d.errors.map((text) => ({ level: 'error', text })),
			...d.warnings.map((text) => ({ level: 'warn', text }))
		];
		if (d.paused) list.push({ level: 'warn', text: 'paused' });
		if (!d.head_closed && !d.errors.includes('head open'))
			list.push({ level: 'error', text: 'head open' });
		if (d.head_closed && !d.media && !d.errors.includes('media out'))
			list.push({ level: 'warn', text: 'no media detected' });
		return list;
	});

	let shown = $derived(
		(v?.all ?? []).filter((s) =>
			`${s.key} ${s.value}`.toLowerCase().includes((v?.filter ?? '').trim().toLowerCase())
		)
	);

	function open(target: string) {
		if (target.trim()) goto(printerHref(target.trim()));
	}
</script>

<div class="top">
	<h1>{ip || 'Printer'}</h1>
	<select value={ip} onchange={(e) => open(e.currentTarget.value)}>
		<option value="" disabled>Choose a printer…</option>
		{#each printerPage.printers as p}
			<option value={p.ip}>{p.ip}{p.model ? ` (${p.model})` : ''}</option>
		{/each}
	</select>
	<form
		onsubmit={(e) => {
			e.preventDefault();
			open(printerPage.typed);
		}}
	>
		<input bind:value={printerPage.typed} placeholder="or another IP / hostname" />
		<button type="submit" disabled={!printerPage.typed.trim()}>Open</button>
	</form>
	{#if ip}<a class="button" href={diagnoseHref(ip)}>Diagnose</a>{/if}
</div>

{#if !ip}
	<p class="muted">
		Choose a printer, or open one from the Configure page by clicking its address.
	</p>
{:else if v}
	<div class="bar">
		{#if v.details}
			<span class="chip" class:busy={v.details.status === 'busy'}
				>{v.details.status || 'unknown'}</span
			>
		{/if}
		<button onclick={() => refresh(ip)}>Refresh</button>
		<label><input type="checkbox" bind:checked={printerPage.autoRefresh} /> Refresh every 5 s</label>
		{#if v.updated}<span class="muted">Updated {v.updated}</span>{/if}
		{#if v.detailsError}<span class="error">{v.detailsError}</span>{/if}
	</div>

	<section class="card alerts">
		<h2>Alerts</h2>
		{#if !v.details}
			<p class="muted">{v.detailsError ? 'Can’t reach the printer.' : 'Reading…'}</p>
		{:else if alerts.length === 0}
			<p class="ok">No alerts</p>
		{:else}
			<ul>
				{#each alerts as a}<li class={a.level}>{a.text}</li>{/each}
			</ul>
		{/if}
	</section>

	<div class="grid">
		<section class="card">
			<h2>Profile</h2>
			{#if listed}
				<div class="stack">
					<select
						value={chosenProfile}
						onchange={(e) => (picks.profile[ip] = e.currentTarget.value)}
					>
						<option value="" disabled>Choose a profile…</option>
						{#each fitting as pf}<option value={pf.id}>{pf.name}</option>{/each}
					</select>
					{#if folder}
						<label>
							<input
								type="checkbox"
								checked={picks.useFolder[ip] ?? true}
								onchange={(e) => (picks.useFolder[ip] = e.currentTarget.checked)}
							/>
							Send the Zebra folder too
						</label>
					{/if}
					<button
						class="primary"
						disabled={!chosenProfile || listed.status.state === 'Applying'}
						onclick={applyProfile}>Apply profile</button
					>
					<span
						class:error={listed.status.state === 'Failed'}
						class:ok={listed.status.state === 'Applied'}>{statusText(listed.status)}</span
					>
				</div>
			{:else}
				<p class="muted">Profiles can be applied to printers in the Configure list.</p>
				<button onclick={addToList}>Add {ip} to the list</button>
			{/if}
			{#if v.setupError}<p class="error">{v.setupError}</p>{/if}
		</section>

		<section class="card">
			<h2>Calibrate</h2>
			<div class="stack">
				<select value={method} onchange={(e) => v && (v.calMethod = e.currentTarget.value)}>
					<option value="gap">Gap/notch (transmissive sensor)</option>
					<option value="mark">Black mark (reflective sensor)</option>
					<option value="continuous">Continuous</option>
					<option value="auto">Auto-detect</option>
				</select>
				<small class="muted"
					>Also sets the printer's media type to this. Auto-detect can mistake the gaps between
					wristbands for gap/notch media, so pick black mark for marked wristbands.</small
				>
				<button class="primary" disabled={v.calBusy} onclick={calibrate}>
					{v.calBusy ? 'Calibrating…' : 'Calibrate'}
				</button>
				{#if v.calError}<span class="error">{v.calError}</span>{:else if v.calNote}<span
						class="ok">{v.calNote}</span
					>{/if}
			</div>
		</section>

		{#each groups as g}
			<section class="card">
				<h2>{g.title}</h2>
				<dl>
					{#each g.rows as [label, key]}
						<dt title={key}>{label}</dt>
						<dd>{v.details ? show(key, v.details.values[key]) : '…'}</dd>
					{/each}
				</dl>
			</section>
		{/each}
	</div>

	<section class="card">
		<h2>Commands</h2>
		<div class="actions">
			{#each actions as a}
				<button disabled={v.busy !== null} title={a.hint} onclick={() => run(a)}>
					{v.busy === a.id ? `${a.label}…` : a.label}
				</button>
			{/each}
		</div>
		{#if v.actionError}<p class="error">{v.actionError}</p>{:else if v.actionNote}<p
				class="muted"
			>
				{v.actionNote}
			</p>{/if}

		<form
			class="raw"
			onsubmit={(e) => {
				e.preventDefault();
				sendRaw();
			}}
		>
			<input
				bind:value={v.rawCommand}
				placeholder={'Send a command, e.g. ! U1 getvar "device.uptime"'}
			/>
			<button type="submit" disabled={v.rawBusy || !v.rawCommand.trim()}>
				{v.rawBusy ? 'Sending…' : 'Send'}
			</button>
		</form>
		{#if v.rawReply !== null}<pre>{v.rawReply}</pre>{/if}
	</section>

	<section class="card">
		<div class="bar">
			<h2>All settings</h2>
			<input bind:value={v.filter} placeholder="Filter, e.g. wlan or 406" />
			<button onclick={() => loadAll(ip)} disabled={v.allLoading}
				>{v.allLoading ? 'Reading…' : 'Reload'}</button
			>
			{#if v.all}<span class="muted">{shown.length} of {v.all.length}</span>{/if}
		</div>
		{#if v.allError}
			<p class="error">{v.allError}</p>
		{:else if v.all}
			<table class="all">
				<colgroup><col style="width: 38%" /><col style="width: 30%" /><col /></colgroup>
				<thead><tr><th>Setting</th><th>Value</th><th>Choices</th></tr></thead>
				<tbody>
					{#each shown as s (s.key)}
						<tr>
							<td><code>{s.key}</code></td>
							<td>{s.value}</td>
							<td class="muted">{s.choices ?? ''}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{:else}
			<p class="muted">Reading every setting takes a few seconds…</p>
		{/if}
	</section>
{/if}

<style>
	.top,
	.bar {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 10px;
		margin-bottom: 12px;
	}
	.top h1 {
		margin: 0 8px 0 0;
	}
	.top form,
	.raw {
		display: flex;
		gap: 6px;
	}
	.bar h2 {
		margin: 0;
	}
	a.button {
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 5px 12px;
		color: inherit;
		text-decoration: none;
		background: var(--panel);
	}
	.stack {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 8px;
	}
	.stack select {
		max-width: 100%;
	}
	.chip {
		padding: 2px 10px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--ok) 15%, var(--panel));
		color: var(--ok);
		font-weight: 600;
	}
	.chip.busy {
		background: color-mix(in srgb, var(--warn) 15%, var(--panel));
		color: var(--warn);
	}
	.card {
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 12px 16px;
		margin-bottom: 14px;
	}
	.card h2 {
		margin-top: 0;
	}
	.alerts ul {
		margin: 0;
		padding-left: 18px;
	}
	.alerts li {
		font-weight: 600;
		text-transform: capitalize;
	}
	li.error {
		color: var(--danger);
	}
	li.warn {
		color: var(--warn);
	}
	.ok {
		color: var(--ok);
		font-weight: 600;
		margin: 0;
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
		gap: 14px;
		margin-bottom: 14px;
	}
	.grid .card {
		margin: 0;
	}
	dl {
		display: grid;
		grid-template-columns: auto 1fr;
		gap: 4px 12px;
		margin: 0;
	}
	dt {
		color: var(--muted);
	}
	dd {
		margin: 0;
		overflow-wrap: anywhere;
	}
	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin-bottom: 10px;
	}
	.raw input {
		flex: 1;
		font-family: ui-monospace, monospace;
	}
	pre {
		background: var(--bg);
		border: 1px solid var(--border);
		border-radius: 6px;
		padding: 8px;
		max-height: 240px;
		overflow: auto;
		white-space: pre-wrap;
	}
	table.all {
		table-layout: fixed;
		border: none;
	}
	table.all td {
		overflow-wrap: anywhere;
	}
</style>
