<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import { onMount } from 'svelte';
	import { follow, picks, selected } from '$lib/configure.svelte';
	import { findPrinter, finding } from '$lib/find.svelte';
	import { onShortcut } from '$lib/shortcuts';
	import { dragSelect } from '$lib/selection';
	import {
		fits,
		flashHint,
		folderFor,
		folderLabel,
		printerHref,
		statusText,
		type FolderView,
		type Moved,
		type Printer,
		type Profile
	} from '$lib/types';

	let printers = $state<Printer[]>([]);
	let profiles = $state<Profile[]>([]);
	let folders = $state<FolderView[]>([]);
	let ip = $state('');
	let addressBox = $state<HTMLInputElement>();
	let adding = $state<string | null>(null); // what's being added, as typed
	let error = $state('');
	let note = $state(''); // how a control number led to the printer just added

	let bulkProfile = $state('');
	let bulkFolder = $state(true);

	// printers given a profile from their Printer page aren't in this list
	const listed = async () =>
		(await invoke<Printer[]>('list_printers')).filter((p) => !p.hidden);

	onMount(() => {
		(async () => {
			printers = await listed();
			profiles = (await invoke<{ profiles: Profile[] }>('list_profiles')).profiles;
			folders = await invoke<FolderView[]>('list_folders');
		})();
		const un = listen<Printer>('printer-update', (e) => {
			printers = printers.map((x) => (x.ip === e.payload.ip ? e.payload : x));
		});
		const unMoved = listen<Moved>('printer-moved', (e) => {
			const { from, printer } = e.payload;
			printers = printers.map((x) => (x.ip === from ? printer : x));
			follow(from, printer.ip);
		});
		const offs = [
			onShortcut('refresh', async () => (printers = await listed())),
			onShortcut('find', () => addressBox?.focus())
		];
		return () => {
			un.then((f) => f());
			unMoved.then((f) => f());
			offs.forEach((off) => off());
		};
	});

	// what a printer also goes by, besides the address it's listed under
	const alsoKnownAs = (p: Printer) =>
		[p.hostname, p.ip_addr].filter(
			(a): a is string => !!a && a.toLowerCase() !== p.ip.toLowerCase()
		);

	let chosen = $derived(printers.filter((p) => selected.has(p.ip)));
	// only profiles that fit every selected printer
	let common = $derived(profiles.filter((pf) => chosen.every((p) => fits(pf.models, p.model))));
	$effect(() => {
		if (bulkProfile && !common.some((pf) => pf.id === bulkProfile)) bulkProfile = '';
	});

	// by its address, or by its control number, which is looked up in ServiceNow
	async function add() {
		error = note = '';
		const typed = ip.trim();
		adding = typed;
		try {
			const found = await findPrinter(typed);
			const p = await invoke<Printer>('add_printer', { ip: found.address });
			printers = [...printers, p];
			note = found.note ?? '';
			if (ip.trim() === typed) ip = '';
		} catch (e) {
			error = String(e);
		}
		adding = null;
	}

	async function start(p: Printer, profileId: string, useFolder: boolean) {
		await invoke('apply_profile', { ip: p.ip, profileId, useFolder });
	}

	async function apply(p: Printer) {
		const profileId = picks.profile[p.ip] ?? p.profile_id;
		if (!profileId) {
			error = `Choose a profile for ${p.ip} first`;
			return;
		}
		error = '';
		try {
			await start(p, profileId, picks.useFolder[p.ip] ?? true);
		} catch (e) {
			error = String(e);
		}
	}

	async function applySelected() {
		const busy = chosen.filter((p) => p.status.state === 'Applying');
		const problems = busy.length ? [`skipped ${busy.length} already applying`] : [];
		for (const p of chosen.filter((p) => p.status.state !== 'Applying')) {
			picks.profile[p.ip] = bulkProfile;
			picks.useFolder[p.ip] = bulkFolder;
			try {
				await start(p, bulkProfile, bulkFolder);
			} catch (e) {
				problems.push(`${p.ip}: ${e}`);
			}
		}
		error = problems.join('; ');
	}

	// drag across rows to select them; Shift or Ctrl adds or removes rows from the selection
	let drag: { anchor: number; base: Set<string>; add: boolean } | null = null;

	function rowDown(e: PointerEvent, i: number) {
		if (e.button !== 0 || (e.target as HTMLElement).closest('a, select, button, input, label')) return;
		e.preventDefault(); // no text selection while dragging
		const keep = e.shiftKey || e.ctrlKey || e.metaKey;
		drag = {
			anchor: i,
			base: keep ? new Set(selected) : new Set(),
			add: !(keep && selected.has(printers[i].ip))
		};
		paint(i);
	}

	function paint(to: number) {
		if (!drag) return;
		const rows = printers.map((p) => p.ip);
		const next = dragSelect(drag.base, rows, drag.anchor, to, drag.add);
		selected.clear();
		next.forEach((ip) => selected.add(ip));
	}

	function toggle(ip: string, on: boolean) {
		if (on) selected.add(ip);
		else selected.delete(ip);
	}

	function toggleAll(on: boolean) {
		selected.clear();
		if (on) printers.forEach((p) => selected.add(p.ip));
	}
</script>

<svelte:window
	onpointerup={() => (drag = null)}
	onkeydown={(e) => e.key === 'Escape' && selected.clear()}
/>

<h1>Configure printers</h1>

<form
	onsubmit={(e) => {
		e.preventDefault();
		add();
	}}
>
	<input
		bind:this={addressBox}
		bind:value={ip}
		placeholder="IP address, hostname or control number"
		size="36"
	/>
	<button type="submit" class="primary" disabled={adding !== null || !ip.trim()}>
		{adding === null ? 'Add printer' : adding in finding ? 'Looking up…' : 'Connecting…'}
	</button>
	{#if adding !== null && finding[adding]}<span class="muted">{finding[adding]}</span>{/if}
</form>
{#if note}<p class="ok">{note}</p>{/if}

{#if printers.length === 0}
	<p class="muted">
		Add a printer by its IP address, hostname or control number to start. A control number, like
		722858, is looked up in ServiceNow.
	</p>
	{#if error}<p class="error">{error}</p>{/if}
{:else}
	<div class="bulk">
		{#if selected.size === 0}
			<span class="muted">Drag across rows to select printers, or Shift-click to pick some.</span>
		{:else}
			<strong>{selected.size} selected</strong>
			<select bind:value={bulkProfile}>
				<option value="" disabled>Profile…</option>
				{#each common as pf}<option value={pf.id}>{pf.name}</option>{/each}
			</select>
			<label title={flashHint}
				><input type="checkbox" bind:checked={bulkFolder} /> Flash from the Zebra folder (as if by USB
				stick), where one matches</label
			>
			<button class="primary" disabled={!bulkProfile} onclick={applySelected}
				>Apply to selected</button
			>
			<button onclick={() => selected.clear()}>Clear selection</button>
			{#if common.length === 0}<span class="muted">No profile fits all of these models.</span>{/if}
		{/if}
	</div>
	{#if error}<p class="error">{error}</p>{/if}

	<table>
		<!-- fixed widths, so a changing status doesn't resize the other columns -->
		<colgroup>
			<col style="width: 36px" />
			<col style="width: 15%" />
			<col style="width: 8%" />
			<col style="width: 12%" />
			<col style="width: 10%" />
			<col style="width: 15%" />
			<col style="width: 11%" />
			<col />
			<col style="width: 108px" />
		</colgroup>
		<thead>
			<tr>
				<th>
					<input
						type="checkbox"
						aria-label="Select all"
						checked={selected.size === printers.length}
						indeterminate={selected.size > 0 && selected.size < printers.length}
						onchange={(e) => toggleAll(e.currentTarget.checked)}
					/>
				</th>
				<th>Printer</th><th>Model</th><th>Serial</th><th>Firmware</th><th>Profile</th>
				<th title={flashHint}>Flash (as if by USB)</th><th>Status</th><th></th>
			</tr>
		</thead>
		<tbody>
			{#each printers as p, i (p.ip)}
				{@const folder = folderFor(folders, p.model)}
				{@const aka = alsoKnownAs(p)}
				<tr
					class:selected={selected.has(p.ip)}
					onpointerdown={(e) => rowDown(e, i)}
					onpointerenter={() => paint(i)}
				>
					<td>
						<input
							type="checkbox"
							aria-label="Select {p.ip}"
							checked={selected.has(p.ip)}
							onchange={(e) => toggle(p.ip, e.currentTarget.checked)}
						/>
					</td>
					<td>
						<a href={printerHref(p.ip)} title="Open this printer's page">{p.ip}</a>
						{#if aka.length}<br /><small class="muted" title="Its hostname and IP"
								>{aka.join(' · ')}</small
							>{/if}
					</td>
					<td>{p.model ?? '?'}</td>
					<td>{p.serial ?? '?'}</td>
					<td>{p.firmware ?? '?'}</td>
					<td>
						<select
							value={picks.profile[p.ip] ?? p.profile_id ?? ''}
							onchange={(e) => (picks.profile[p.ip] = e.currentTarget.value)}
						>
							<option value="" disabled>Choose…</option>
							{#each profiles.filter((x) => fits(x.models, p.model)) as pf}
								<option value={pf.id}>{pf.name}</option>
							{/each}
						</select>
					</td>
					<td>
						{#if folder}
							<label title={`${flashHint}\n\n${folder.path}`}>
								<input
									type="checkbox"
									checked={picks.useFolder[p.ip] ?? true}
									onchange={(e) => (picks.useFolder[p.ip] = e.currentTarget.checked)}
								/>
								{folderLabel(folder)}
							</label>
						{:else}
							<span class="muted">no Zebra folder for this model</span>
						{/if}
					</td>
					<td class:error={p.status.state === 'Failed'} class:ok={p.status.state === 'Applied'}>
						{statusText(p.status)}
					</td>
					<td>
						<!-- applying again mid-way would cut off whatever is being sent, even firmware -->
						<button disabled={p.status.state === 'Applying'} onclick={() => apply(p)}>
							{p.status.state === 'Offline' ? 'Retry now' : 'Apply'}
						</button>
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
{/if}

<style>
	form {
		display: flex;
		gap: 8px;
		margin-bottom: 12px;
	}
	.bulk {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 8px;
		min-height: 34px;
		margin-bottom: 8px;
	}
	table {
		table-layout: fixed;
	}
	td {
		overflow-wrap: anywhere;
	}
	td select {
		width: 100%;
	}
	td button {
		white-space: nowrap;
	}
	tr.selected td {
		background: color-mix(in srgb, var(--accent) 14%, var(--panel));
	}
	td.ok {
		color: var(--ok);
	}
</style>
