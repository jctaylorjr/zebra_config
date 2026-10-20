<script lang="ts">
	import { getVersion } from '@tauri-apps/api/app';
	import { invoke } from '@tauri-apps/api/core';
	import { confirm } from '@tauri-apps/plugin-dialog';
	import { onMount } from 'svelte';
	import { recent } from '$lib/recent.svelte';
	import { prefs, setTheme, type Theme } from '$lib/theme.svelte';
	import { notify } from '$lib/toast.svelte';

	let version = $state('');
	let error = $state('');

	onMount(async () => {
		version = await getVersion();
	});

	const themes: { id: Theme; label: string }[] = [
		{ id: 'system', label: 'System' },
		{ id: 'light', label: 'Light' },
		{ id: 'dark', label: 'Dark' }
	];

	const files = [
		['profiles.json', 'Profiles, edited on the Profiles page'],
		['settings.json', 'Zebra folders and these preferences'],
		['recent.json', 'Recent printers on the Printer and Diagnose pages']
	];

	const shortcuts = [
		['Ctrl+1 to Ctrl+6', 'Go to Configure, Printer, Diagnose, Profiles, Clone or Settings'],
		['Alt+← and Alt+→', 'Back and forward; the mouse’s back and forward buttons too'],
		['F5 or Ctrl+R', 'Refresh: re-reads the printer on the Printer page, the list on Configure'],
		['Ctrl+F', 'Jump to the filter on the Printer page, or the address box on Configure'],
		['Ctrl+S', 'Save profile changes'],
		['Ctrl+Enter', 'Run the diagnosis'],
		['Esc', 'Clear the printer selection on Configure']
	];

	async function openFolder() {
		error = '';
		try {
			await invoke('open_data_folder');
		} catch (e) {
			error = String(e);
		}
	}

	async function clearRecent() {
		const ok = await confirm('Forget every recent printer?', {
			title: 'Clear recent printers',
			okLabel: 'Clear'
		});
		if (!ok) return;
		try {
			await invoke('clear_recent');
			recent.list = [];
			notify('Cleared the recent printers');
		} catch (e) {
			error = String(e);
		}
	}
</script>

<h1>Settings</h1>
{#if error}<p class="error">{error}</p>{/if}

<section class="card">
	<h2>Appearance</h2>
	<div class="choices" role="radiogroup" aria-label="Theme">
		{#each themes as t}
			<label class:active={prefs.theme === t.id}>
				<input
					type="radio"
					name="theme"
					checked={prefs.theme === t.id}
					onchange={() => setTheme(t.id)}
				/>
				{t.label}
			</label>
		{/each}
	</div>
	<p class="muted small">System follows the computer’s light or dark mode.</p>
</section>

<section class="card">
	<h2>Data</h2>
	<p>
		Everything is saved on this computer in
		<code>{prefs.dataDir || '…'}</code>
	</p>
	<button onclick={openFolder}>Open data folder</button>
	<table>
		<tbody>
			{#each files as [name, about]}
				<tr><td><code>{name}</code></td><td>{about}</td></tr>
			{/each}
		</tbody>
	</table>
	<div class="row">
		<button disabled={recent.list.length === 0} onclick={clearRecent}>Clear recent printers</button>
		<span class="muted">{recent.list.length} saved</span>
	</div>
	<p class="muted small">To share profiles, use Export and Import on the Profiles page.</p>
</section>

<section class="card">
	<h2>Keyboard shortcuts</h2>
	<table>
		<tbody>
			{#each shortcuts as [keys, what]}
				<tr><td class="keys"><kbd>{keys}</kbd></td><td>{what}</td></tr>
			{/each}
		</tbody>
	</table>
</section>

<section class="card">
	<h2>About</h2>
	<p>Zebra Configure {version}</p>
</section>

<style>
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
	.card table {
		margin: 12px 0;
	}
	.choices {
		display: flex;
		gap: 8px;
	}
	.choices label {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 6px 14px;
		border: 1px solid var(--border);
		border-radius: 6px;
		cursor: pointer;
	}
	.choices label.active {
		border-color: var(--accent);
		background: color-mix(in srgb, var(--accent) 12%, var(--panel));
	}
	.row {
		display: flex;
		align-items: center;
		gap: 10px;
	}
	.small {
		font-size: 12px;
		margin: 8px 0 0;
	}
	.keys {
		width: 200px;
	}
	kbd {
		font: 12px ui-monospace, monospace;
		padding: 2px 6px;
		border: 1px solid var(--border);
		border-bottom-width: 2px;
		border-radius: 4px;
		background: var(--bg);
	}
	code {
		overflow-wrap: anywhere;
	}
</style>
