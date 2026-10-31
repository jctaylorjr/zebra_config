<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { listen } from '@tauri-apps/api/event';
	import { onMount } from 'svelte';
	import { clonePage as c } from '$lib/clone.svelte';
	import { findPrinter, finding } from '$lib/find.svelte';
	import { recent } from '$lib/recent.svelte';
	import {
		flashHint,
		printerHref,
		recentLabel,
		statusText,
		type CloneSource,
		type Moved,
		type Printer
	} from '$lib/types';

	let printers = $state<Printer[]>([]); // the app's printers, to pick from and to follow
	let reading = $state(false);
	let starting = $state(false);
	let error = $state('');
	// how a control number typed in either box led to its printer
	let fromNote = $state('');
	let toNote = $state('');
	let looking = $state<string | null>(null); // a control number being looked up, as typed

	onMount(() => {
		(async () => (printers = await invoke<Printer[]>('list_printers')))();
		const un = listen<Printer>('printer-update', (e) => {
			printers = printers.map((x) => (x.ip === e.payload.ip ? e.payload : x));
		});
		const unMoved = listen<Moved>('printer-moved', (e) => {
			const { from, printer } = e.payload;
			printers = printers.map((x) => (x.ip === from ? printer : x));
			if (c.target === from) c.target = printer.ip;
		});
		return () => {
			un.then((f) => f());
			unMoved.then((f) => f());
		};
	});

	// the printer being copied to, as Configure lists it
	let target = $derived(printers.find((p) => p.ip === c.target));
	let busy = $derived(target?.status.state === 'Applying' || target?.status.state === 'Offline');
	// the Configure list, to pick from; printers given a profile from their Printer page aren't in it
	let listed = $derived(printers.filter((p) => !p.hidden));
	let recentOthers = $derived(
		recent.list.filter(
			(r) => !listed.some((p) => p.ip.toLowerCase() === r.address.toLowerCase())
		)
	);
	let copyingName = $derived(c.pick.includes('hostname') ? c.source?.hostname : null);

	// the address for what's in a box, a control number looked up and put in its place
	async function lookUp(typed: string) {
		looking = typed.trim();
		try {
			return await findPrinter(typed);
		} finally {
			looking = null;
		}
	}

	async function read() {
		error = fromNote = '';
		reading = true;
		try {
			const found = await lookUp(c.from);
			c.from = found.address;
			fromNote = found.note ?? '';
			c.source = await invoke<CloneSource>('clone_source', { ip: found.address });
			c.pick = c.source.settings.map((s) => s.id);
		} catch (e) {
			c.source = null;
			error = String(e);
		}
		reading = false;
	}

	async function clone() {
		error = toNote = '';
		starting = true;
		try {
			const found = await lookUp(c.to);
			c.to = found.address;
			toNote = found.note ?? '';
			const followed = await invoke<string>('clone_printer', {
				from: c.from,
				to: found.address,
				pick: c.pick,
				useFolder: c.useFolder
			});
			printers = await invoke<Printer[]>('list_printers');
			c.target = followed;
		} catch (e) {
			error = String(e);
		}
		starting = false;
	}

	const toggle = (id: string, on: boolean) =>
		(c.pick = on ? [...c.pick, id] : c.pick.filter((x) => x !== id));
</script>

<h1>Clone</h1>
<p class="muted intro">
	Copies one printer's hostname and print settings to another, such as a loaner standing in for a
	printer that's away. The hostname is copied as it is, even though it holds the other printer's
	serial. Applying then goes as on Configure: the printer is restarted so its new name takes
	effect, followed, and read back; with the media copied, it's calibrated and prints a test label.
</p>
{#if error}<p class="error">{error}</p>{/if}

<section class="card">
	<h2>Copy from</h2>
	<form
		onsubmit={(e) => {
			e.preventDefault();
			read();
		}}
	>
		<input
			list="clone-printers"
			bind:value={c.from}
			placeholder="IP, hostname or control number"
		/>
		<button type="submit" disabled={!c.from.trim() || reading}>
			{reading ? (looking !== null ? 'Looking up…' : 'Reading…') : 'Read settings'}
		</button>
		{#if reading && looking !== null && finding[looking]}
			<span class="muted">{finding[looking]}</span>
		{/if}
	</form>
	{#if fromNote}<p class="ok small">{fromNote}</p>{/if}
	{#if c.source}
		<p class="who">
			<strong>{c.source.hostname ?? c.from}</strong>
			<span class="muted">
				{[c.source.ip_addr, c.source.model, c.source.serial].filter(Boolean).join(' · ')}
			</span>
		</p>
		<table>
			<thead>
				<tr>
					<th class="check">
						<input
							type="checkbox"
							aria-label="Copy every setting"
							checked={c.pick.length === c.source.settings.length}
							indeterminate={c.pick.length > 0 && c.pick.length < c.source.settings.length}
							onchange={(e) =>
								(c.pick = e.currentTarget.checked && c.source
									? c.source.settings.map((s) => s.id)
									: [])}
						/>
					</th>
					<th>Setting</th>
					<th>Value</th>
				</tr>
			</thead>
			<tbody>
				{#each c.source.settings as s}
					<tr>
						<td class="check">
							<input
								type="checkbox"
								aria-label="Copy {s.label}"
								checked={c.pick.includes(s.id)}
								onchange={(e) => toggle(s.id, e.currentTarget.checked)}
							/>
						</td>
						<td>{s.label}</td>
						<td>{s.value}</td>
					</tr>
				{/each}
			</tbody>
		</table>
		<p class="muted small">Read again just before cloning, so the copy matches it as it is now.</p>
	{/if}
</section>

<section class="card">
	<h2>Copy to</h2>
	<div class="row">
		<input
			list="clone-printers"
			bind:value={c.to}
			placeholder="IP, hostname or control number of the loaner"
		/>
		<label title={flashHint}
			><input type="checkbox" bind:checked={c.useFolder} /> Flash it from its model's Zebra folder too,
			as if you had used that USB stick</label
		>
	</div>
	{#if toNote}<p class="ok small">{toNote}</p>{/if}
	{#if copyingName}
		<p class="warn">
			The printer copied from is still called {copyingName}, and two printers can't share a name:
			WINS and DNS keep it for whichever had it first. Take that printer off the network, then
			restart this one from its Printer page so it takes the name over.
		</p>
	{/if}
	<div class="row">
		<button
			class="primary"
			disabled={!c.source || !c.to.trim() || c.pick.length === 0 || starting || busy}
			onclick={clone}
		>
			{starting ? (looking !== null ? 'Looking up…' : 'Starting…') : 'Clone'}
		</button>
		{#if starting && looking !== null && finding[looking]}
			<span class="muted">{finding[looking]}</span>
		{/if}
		{#if target}
			<span>
				<a href={printerHref(target.ip)}>{target.hostname ?? target.ip}</a>
				{#if target.hostname && target.hostname.toLowerCase() !== target.ip.toLowerCase()}
					<span class="muted">{target.ip}</span>
				{/if}
				<span
					class:error={target.status.state === 'Failed'}
					class:ok={target.status.state === 'Applied'}
				>
					{statusText(target.status)}
				</span>
			</span>
		{/if}
	</div>
	<p class="muted small">It's added to the Configure list, where its progress shows too.</p>
</section>

<datalist id="clone-printers">
	{#each listed as p}<option value={p.ip}>{p.hostname ?? ''} {p.model ?? ''}</option>{/each}
	{#each recentOthers as r}<option value={r.address}>{recentLabel(r)}</option>{/each}
</datalist>

<style>
	.intro {
		max-width: 760px;
	}
	.card {
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 12px 16px;
		margin-bottom: 14px;
		max-width: 760px;
	}
	.card h2 {
		margin-top: 0;
	}
	form,
	.row {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 10px;
	}
	.row + .row {
		margin-top: 10px;
	}
	input:not([type='checkbox']) {
		min-width: 260px;
	}
	.who {
		margin: 12px 0 8px;
	}
	.check {
		width: 28px;
	}
	.small {
		font-size: 12px;
		margin: 8px 0 0;
	}
	.warn {
		color: var(--warn);
	}
	.ok {
		color: var(--ok);
	}
</style>
