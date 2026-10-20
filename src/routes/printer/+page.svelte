<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { invoke } from '@tauri-apps/api/core';
	import { confirm } from '@tauri-apps/plugin-dialog';
	import { onMount, untrack } from 'svelte';
	import { printerHref, type Details, type Printer, type PrinterSetting } from '$lib/types';

	let ip = $derived(page.url.searchParams.get('ip') ?? '');
	let printers = $state<Printer[]>([]);
	let typed = $state('');

	let details = $state<Details | null>(null);
	let detailsError = $state('');
	let updated = $state('');
	let autoRefresh = $state(true);

	let all = $state<PrinterSetting[] | null>(null);
	let allError = $state('');
	let allLoading = $state(false);
	let filter = $state('');

	let busy = $state<string | null>(null);
	let actionNote = $state('');
	let actionError = $state('');

	let rawCommand = $state('');
	let rawReply = $state<string | null>(null);
	let rawBusy = $state(false);

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

	const show = (key: string, v: unknown) => {
		if (v === null || v === undefined) return key === 'ip.hostname' ? 'not on this firmware' : '—';
		const s = String(v);
		if (key === 'ip.primary_network') return s === '1' ? 'Wired' : s === '2' ? 'Wireless' : s;
		if (key === 'wlan.signal_strength' && /^\d+$/.test(s)) return `${s}%`;
		return s || '—';
	};

	// feeding commands check for media first; the rest just go
	const actions: { id: string; label: string; feeds?: boolean; ask?: string; hint?: string }[] = [
		{ id: 'calibrate', label: 'Calibrate', feeds: true },
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

	onMount(async () => {
		printers = await invoke<Printer[]>('list_printers');
	});

	async function refresh() {
		const target = ip;
		if (!target) return;
		try {
			const d = await invoke<Details>('printer_details', { ip: target, keys });
			if (target !== ip) return;
			details = d;
			detailsError = '';
			updated = new Date().toLocaleTimeString();
		} catch (e) {
			if (target === ip) detailsError = String(e);
		}
	}

	async function loadAll() {
		const target = ip;
		if (!target) return;
		allLoading = true;
		allError = '';
		try {
			const r = await invoke<PrinterSetting[]>('printer_settings', { ip: target });
			if (target === ip) all = r;
		} catch (e) {
			if (target === ip) allError = String(e);
		}
		allLoading = false;
	}

	// start over whenever a different printer is opened
	$effect(() => {
		ip;
		untrack(() => {
			details = null;
			detailsError = '';
			all = null;
			rawReply = null;
			actionNote = actionError = '';
			refresh();
			loadAll();
		});
	});

	$effect(() => {
		if (!ip || !autoRefresh) return;
		const timer = setInterval(refresh, 5000);
		return () => clearInterval(timer);
	});

	let alerts = $derived.by(() => {
		if (!details) return [];
		const list = [
			...details.errors.map((text) => ({ level: 'error', text })),
			...details.warnings.map((text) => ({ level: 'warn', text }))
		];
		if (details.paused) list.push({ level: 'warn', text: 'paused' });
		if (!details.head_closed && !details.errors.includes('head open'))
			list.push({ level: 'error', text: 'head open' });
		if (details.head_closed && !details.media && !details.errors.includes('media out'))
			list.push({ level: 'warn', text: 'no media detected' });
		return list;
	});

	let shown = $derived(
		(all ?? []).filter((s) =>
			`${s.key} ${s.value}`.toLowerCase().includes(filter.trim().toLowerCase())
		)
	);

	function open(target: string) {
		if (target.trim()) goto(printerHref(target.trim()));
	}

	async function run(a: (typeof actions)[number]) {
		if (a.feeds && details && !(details.media && details.head_closed)) {
			const why = details.head_closed ? 'doesn’t see any media' : 'has its head open';
			const ok = await confirm(
				`The printer ${why}. Feeding now would leave it in an error. Send anyway?`,
				{ title: a.label }
			);
			if (!ok) return;
		}
		if (a.ask && !(await confirm(a.ask, { title: a.label }))) return;
		busy = a.id;
		actionNote = actionError = '';
		try {
			actionNote = `${a.label}: ${await invoke<string>('printer_action', { ip, action: a.id })}`;
		} catch (e) {
			actionError = `${a.label}: ${e}`;
		}
		busy = null;
		refresh();
	}

	async function sendRaw() {
		if (!rawCommand.trim()) return;
		rawBusy = true;
		try {
			rawReply = (await invoke<string>('printer_raw', { ip, command: rawCommand })) || '(no reply)';
		} catch (e) {
			rawReply = `Failed: ${e}`;
		}
		rawBusy = false;
	}
</script>

<div class="top">
	<h1>{ip || 'Printer'}</h1>
	<select value={ip} onchange={(e) => open(e.currentTarget.value)}>
		<option value="" disabled>Choose a printer…</option>
		{#each printers as p}
			<option value={p.ip}>{p.ip}{p.model ? ` (${p.model})` : ''}</option>
		{/each}
	</select>
	<form
		onsubmit={(e) => {
			e.preventDefault();
			open(typed);
		}}
	>
		<input bind:value={typed} placeholder="or another IP / hostname" />
		<button type="submit" disabled={!typed.trim()}>Open</button>
	</form>
</div>

{#if !ip}
	<p class="muted">
		Choose a printer, or open one from the Configure page by clicking its address.
	</p>
{:else}
	<div class="bar">
		{#if details}
			<span class="chip" class:busy={details.status === 'busy'}>{details.status || 'unknown'}</span>
		{/if}
		<button onclick={refresh}>Refresh</button>
		<label><input type="checkbox" bind:checked={autoRefresh} /> Refresh every 5 s</label>
		{#if updated}<span class="muted">Updated {updated}</span>{/if}
		{#if detailsError}<span class="error">{detailsError}</span>{/if}
	</div>

	<section class="card alerts">
		<h2>Alerts</h2>
		{#if !details}
			<p class="muted">{detailsError ? 'Can’t reach the printer.' : 'Reading…'}</p>
		{:else if alerts.length === 0}
			<p class="ok">No alerts</p>
		{:else}
			<ul>
				{#each alerts as a}<li class={a.level}>{a.text}</li>{/each}
			</ul>
		{/if}
	</section>

	<div class="grid">
		{#each groups as g}
			<section class="card">
				<h2>{g.title}</h2>
				<dl>
					{#each g.rows as [label, key]}
						<dt title={key}>{label}</dt>
						<dd>{details ? show(key, details.values[key]) : '…'}</dd>
					{/each}
				</dl>
			</section>
		{/each}
	</div>

	<section class="card">
		<h2>Commands</h2>
		<div class="actions">
			{#each actions as a}
				<button disabled={busy !== null} title={a.hint} onclick={() => run(a)}>
					{busy === a.id ? `${a.label}…` : a.label}
				</button>
			{/each}
		</div>
		{#if actionError}<p class="error">{actionError}</p>{:else if actionNote}<p class="muted">
				{actionNote}
			</p>{/if}

		<form
			class="raw"
			onsubmit={(e) => {
				e.preventDefault();
				sendRaw();
			}}
		>
			<input bind:value={rawCommand} placeholder={'Send a command, e.g. ! U1 getvar "device.uptime"'} />
			<button type="submit" disabled={rawBusy || !rawCommand.trim()}>
				{rawBusy ? 'Sending…' : 'Send'}
			</button>
		</form>
		{#if rawReply !== null}<pre>{rawReply}</pre>{/if}
	</section>

	<section class="card">
		<div class="bar">
			<h2>All settings</h2>
			<input bind:value={filter} placeholder="Filter, e.g. wlan or 406" />
			<button onclick={loadAll} disabled={allLoading}>{allLoading ? 'Reading…' : 'Reload'}</button>
			{#if all}<span class="muted">{shown.length} of {all.length}</span>{/if}
		</div>
		{#if allError}
			<p class="error">{allError}</p>
		{:else if all}
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
