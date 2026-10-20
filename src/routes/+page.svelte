<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import { open } from '@tauri-apps/plugin-dialog';
	import { onMount } from 'svelte';

	type Status =
		| { state: 'Pending' }
		| { state: 'Applying' | 'Applied' | 'Unverified' | 'Failed'; detail: string }
		| { state: 'Offline'; detail: { attempt: number; retry_in_secs: number; reason: string } };
	type Printer = {
		ip: string;
		model: string | null;
		serial: string | null;
		firmware: string | null;
		profile_id: string | null;
		status: Status;
	};
	type Profile = {
		id: string;
		name: string;
		models: string[];
		media: {
			width_in: number | null;
			media_type: 'gap' | 'mark';
			sensor: 'transmissive' | 'reflective';
		} | null;
		commands: string[];
	};
	type FolderSetting = {
		models: string[];
		path: string | null;
		firmware_version: string | null;
	};
	type FolderView = FolderSetting & {
		summary: {
			firmware: string | null;
			files: number;
			commands: number;
			checks: string[];
			error: string | null;
		} | null;
	};

	let printers = $state<Printer[]>([]);
	let profiles = $state<Profile[]>([]);
	let folders = $state<FolderView[]>([]);
	let choice = $state<Record<string, string>>({});
	let ip = $state('');
	let error = $state('');

	onMount(() => {
		(async () => {
			printers = await invoke<Printer[]>('list_printers');
			profiles = await invoke<Profile[]>('list_profiles');
			folders = await invoke<FolderView[]>('list_folders');
		})();
		const un = listen<Printer>('printer-update', (e) => {
			printers = printers.map((x) => (x.ip === e.payload.ip ? e.payload : x));
		});
		return () => {
			un.then((f) => f());
		};
	});

	const fits = (pr: Printer, p: Profile) =>
		p.models.length === 0 || p.models.some((m) => pr.model?.includes(m));

	const label = (s: Status) =>
		s.state === 'Offline'
			? `Offline (${s.detail.reason}): attempt ${s.detail.attempt}, retry in ${s.detail.retry_in_secs}s`
			: s.state === 'Failed'
				? `Failed: ${s.detail}`
				: s.state === 'Applied'
					? `Applied, checked ${s.detail}`
				: s.state === 'Unverified'
					? `Sent, not verified: ${s.detail}`
					: s.state === 'Applying'
						? `${s.detail}…`
						: s.state;

	const profileName = (id: string | null) => profiles.find((x) => x.id === id)?.name ?? id;

	async function add() {
		error = '';
		try {
			const p = await invoke<Printer>('add_printer', { ip });
			printers = [...printers.filter((x) => x.ip !== p.ip), p];
			ip = '';
		} catch (e) {
			error = String(e);
		}
	}

	async function saveFolders(next: FolderSetting[]) {
		error = '';
		try {
			folders = await invoke<FolderView[]>('save_folders', { folders: next });
		} catch (e) {
			error = String(e);
			folders = await invoke<FolderView[]>('list_folders'); // undo the edit in the inputs
		}
	}

	const settingsOf = () =>
		folders.map(({ models, path, firmware_version }) => ({ models, path, firmware_version }));

	function changeFolder(i: number, change: Partial<FolderSetting>) {
		const next = settingsOf();
		next[i] = { ...next[i], ...change };
		saveFolders(next);
	}

	async function chooseFolder(i: number) {
		const path = await open({
			directory: true,
			title: 'Choose the Zebra folder',
			defaultPath: folders[i].path ?? undefined
		});
		if (typeof path === 'string') changeFolder(i, { path });
	}

	async function apply(p: Printer) {
		const profileId = choice[p.ip] ?? p.profile_id;
		if (!profileId) return;
		try {
			await invoke('apply_profile', { ip: p.ip, profileId });
		} catch (e) {
			error = String(e);
		}
	}
</script>

<main>
	<h3>Zebra folders</h3>
	<table>
		<thead><tr><th>For models</th><th>Folder</th><th>Firmware version</th><th></th></tr></thead>
		<tbody>
			{#each folders as f, i}
				<tr>
					<td>
						<input
							value={f.models.join(', ')}
							placeholder="e.g. ZT"
							onchange={(e) =>
								changeFolder(i, {
									models: e.currentTarget.value
										.split(',')
										.map((m) => m.trim())
										.filter(Boolean)
								})}
						/>
					</td>
					<td>
						{#if f.path}<code>{f.path}</code>{:else}none{/if}
						<button onclick={() => chooseFolder(i)}>Choose…</button>
						{#if f.path}<button onclick={() => changeFolder(i, { path: null })}>Clear</button>{/if}
						{#if f.summary?.error}
							<div style="color: crimson">{f.summary.error}</div>
						{:else if f.summary}
							<div>
								{f.summary.firmware ? `firmware ${f.summary.firmware}` : 'no firmware'}, {f.summary
									.files} files, {f.summary.commands} command files{#if f.summary.checks.length};
									checks {f.summary.checks.join(', ')} on E:{/if}
							</div>
						{/if}
					</td>
					<td>
						<input
							value={f.firmware_version ?? ''}
							placeholder="blank = always install"
							onchange={(e) => changeFolder(i, { firmware_version: e.currentTarget.value.trim() || null })}
						/>
					</td>
					<td>
						<button onclick={() => saveFolders(settingsOf().filter((_, j) => j !== i))}>Remove</button>
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
	<button
		onclick={() => saveFolders([...settingsOf(), { models: [], path: null, firmware_version: null }])}
		>Add folder</button
	>

	<h3>Printers</h3>
	<form
		onsubmit={(e) => {
			e.preventDefault();
			add();
		}}
	>
		<input bind:value={ip} placeholder="Printer IP or hostname, e.g. 192.168.1.50" />
		<button type="submit">Add printer</button>
	</form>
	{#if error}<p style="color: crimson">{error}</p>{/if}

	<table>
		<thead
			><tr
				><th>IP</th><th>Model</th><th>Serial</th><th>Firmware</th><th>Profile</th><th>Status</th
				><th></th></tr
			></thead
		>
		<tbody>
			{#each printers as p (p.ip)}
				<tr>
					<td>{p.ip}</td>
					<td>{p.model ?? '?'}</td>
					<td>{p.serial ?? '?'}</td>
					<td>{p.firmware ?? '?'}</td>
					<td>
						<select bind:value={choice[p.ip]}>
							<option value={undefined}>{profileName(p.profile_id) ?? 'choose…'}</option>
							{#each profiles.filter((x) => fits(p, x)) as pf}
								<option value={pf.id}>{pf.name}</option>
							{/each}
						</select>
					</td>
					<td>{label(p.status)}</td>
					<td>
						<!-- applying again mid-way would cut off whatever is being sent, even firmware -->
						<button disabled={p.status.state === 'Applying'} onclick={() => apply(p)}
							>Apply / Retry now</button
						>
					</td>
				</tr>
			{/each}
		</tbody>
	</table>
</main>
