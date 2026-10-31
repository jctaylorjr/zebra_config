<script lang="ts">
	import { findPrinter, finding } from './find.svelte';

	// the box at the top of the Printer and Diagnose pages. it opens what's typed there, looking a
	// control number up first; `onopen` gets the address and how the number led to it
	let {
		typed = $bindable(),
		onopen
	}: { typed: string; onopen: (address: string, note: string) => void } = $props();

	let looking = $state<string | null>(null); // what's being looked up, as typed
	let error = $state('');

	async function open() {
		const q = typed.trim();
		if (!q) return;
		error = '';
		looking = q;
		try {
			const found = await findPrinter(q);
			onopen(found.address, found.note ?? '');
		} catch (e) {
			error = String(e);
		}
		looking = null;
	}
</script>

<form
	onsubmit={(e) => {
		e.preventDefault();
		open();
	}}
>
	<input bind:value={typed} placeholder="or another IP, hostname or control number" size="34" />
	<button type="submit" disabled={looking !== null || !typed.trim()}>
		{looking === null ? 'Open' : 'Looking up…'}
	</button>
</form>
{#if looking !== null && finding[looking]}<span class="muted">{finding[looking]}</span>{/if}
{#if error}<span class="error">{error}</span>{/if}

<style>
	form {
		display: flex;
		gap: 6px;
	}
</style>
