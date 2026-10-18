<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { onMount } from 'svelte';

	type Printer = {
		ip: string;
		model: string | null;
		serial: string | null;
		status: { state: string };
	};

	let printers = $state<Printer[]>([]);
	let ip = $state('');
	let error = $state('');

	onMount(async () => {
		printers = await invoke<Printer[]>('list_printers');
	});

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
			><tr><th>IP</th><th>Model</th><th>Serial</th><th>Status</th></tr></thead
		>
		<tbody>
			{#each printers as p (p.ip)}
				<tr>
					<td>{p.ip}</td>
					<td>{p.model ?? '?'}</td>
					<td>{p.serial ?? '?'}</td>
					<td>{p.status.state}</td>
				</tr>
			{/each}
		</tbody>
	</table>
</main>
