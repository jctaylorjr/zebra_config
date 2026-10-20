<script lang="ts">
	import { forget, recent } from './recent.svelte';

	// where opening a printer goes, e.g. its Printer page or its Diagnose page
	let { hrefFor }: { hrefFor: (address: string) => string } = $props();

	const ago = (secs: number) => {
		const s = Date.now() / 1000 - secs;
		if (s < 60) return 'just now';
		if (s < 3600) return `${Math.floor(s / 60)} min ago`;
		if (s < 86_400) return `${Math.floor(s / 3600)} h ago`;
		if (s < 7 * 86_400) return `${Math.floor(s / 86_400)} days ago`;
		return new Date(secs * 1000).toLocaleDateString();
	};
</script>

<section class="recent">
	<h2>Recent printers</h2>
	{#if recent.list.length === 0}
		<p class="muted">Printers you open show up here, and stay after the app is closed.</p>
	{:else}
		<table>
			<thead>
				<tr><th>Printer</th><th>Model</th><th>Serial</th><th>Last opened</th><th></th></tr>
			</thead>
			<tbody>
				{#each recent.list as r (r.address)}
					<tr>
						<td>
							<a href={hrefFor(r.address)}>{r.hostname ?? r.address}</a>
							{#if (r.hostname ?? r.address).toLowerCase() !== r.address.toLowerCase()}
								<small class="muted">{r.address}</small>
							{/if}
							{#if r.ip_addr && r.ip_addr !== r.address}<small class="muted">{r.ip_addr}</small>{/if}
						</td>
						<td>{r.model ?? '?'}</td>
						<td>{r.serial ?? '?'}</td>
						<td class="muted">{ago(r.last_used)}</td>
						<td>
							<button
								class="remove"
								title="Remove from recent printers"
								aria-label="Remove {r.address} from recent printers"
								onclick={() => forget(r.address)}>×</button
							>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
</section>

<style>
	.recent {
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 12px 16px;
		max-width: 900px;
	}
	h2 {
		margin-top: 0;
	}
	table {
		border: none;
	}
	td small {
		display: block;
	}
	a {
		color: var(--accent);
		font-weight: 600;
	}
	.remove {
		border: none;
		background: none;
		color: var(--muted);
		font-size: 16px;
		padding: 0 6px;
	}
	.remove:hover {
		color: var(--danger);
	}
</style>
