<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import { onMount } from 'svelte';

	type Status =
		| { state: 'Pending' | 'Applying' | 'Applied' }
		| { state: 'Offline'; detail: { attempt: number; retry_in_secs: number } }
		| { state: 'Failed'; detail: string };
	type Printer = {
		ip: string;
		model: string | null;
		serial: string | null;
		profile_id: string | null;
		status: Status;
	};
	type Profile = {
		id: string;
		name: string;
		models: string[];
		commands: string[];
	};

	let printers = $state<Printer[]>([]);
	let profiles = $state<Profile[]>([]);
	let choice = $state<Record<string, string>>({});
	let ip = $state('');
	let error = $state('');

	onMount(() => {
		(async () => {
			printers = await invoke<Printer[]>('list_printers');
			profiles = await invoke<Profile[]>('list_profiles');
		})();
		const un = listen<[string, Status]>('printer-update', (e) => {
			const [pip, status] = e.payload;
			const p = printers.find((x) => x.ip === pip);
			if (p) p.status = status;
		});
		return () => {
			un.then((f) => f());
		};
	});

	const fits = (pr: Printer, p: Profile) =>
		p.models.length === 0 || p.models.some((m) => pr.model?.includes(m));

	const label = (s: Status) =>
		s.state === 'Offline'
			? `Offline: attempt ${s.detail.attempt}, retry in ${s.detail.retry_in_secs}s`
			: s.state === 'Failed'
				? `Failed: ${s.detail}`
				: s.state;

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
	<form
		onsubmit={(e) => {
			e.preventDefault();
			add();
		}}
	>
		<input bind:value={ip} placeholder="Printer IP, e.g. 192.168.1.50" />
		<button type="submit">Add printer</button>
	</form>
	{#if error}<p style="color: crimson">{error}</p>{/if}

	<table>
		<thead
			><tr
				><th>IP</th><th>Model</th><th>Serial</th><th>Profile</th><th>Status</th
				><th></th></tr
			></thead
		>
		<tbody>
			{#each printers as p (p.ip)}
				<tr>
					<td>{p.ip}</td>
					<td>{p.model ?? '?'}</td>
					<td>{p.serial ?? '?'}</td>
					<td>
						<select bind:value={choice[p.ip]}>
							<option value={undefined}>{p.profile_id ?? 'choose…'}</option>
							{#each profiles.filter((x) => fits(p, x)) as pf}
								<option value={pf.id}>{pf.name}</option>
							{/each}
						</select>
					</td>
					<td>{label(p.status)}</td>
					<td><button onclick={() => apply(p)}>Apply / Retry now</button></td>
				</tr>
			{/each}
		</tbody>
	</table>
</main>
