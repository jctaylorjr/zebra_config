<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { open } from '@tauri-apps/plugin-dialog';
	import { onMount } from 'svelte';
	import type { FolderSetting, FolderView } from '$lib/types';

	let folders = $state<FolderView[]>([]);
	let error = $state('');

	onMount(async () => {
		folders = await invoke<FolderView[]>('list_folders');
	});

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
</script>

<h1>Settings</h1>

<h2>Zebra folders</h2>
<p class="muted">
	A folder laid out like a USB mirror stick (appl, files, commands). Each printer gets the first
	folder whose models match it, so firmware only goes to the models it's meant for.
</p>
{#if error}<p class="error">{error}</p>{/if}
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
					{#if f.path}<code>{f.path}</code>{:else}<span class="muted">none</span>{/if}
					<button onclick={() => chooseFolder(i)}>Choose…</button>
					{#if f.path}<button onclick={() => changeFolder(i, { path: null })}>Clear</button>{/if}
					{#if f.summary?.error}
						<div class="error">{f.summary.error}</div>
					{:else if f.summary}
						<div class="muted">
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
						onchange={(e) =>
							changeFolder(i, { firmware_version: e.currentTarget.value.trim() || null })}
					/>
				</td>
				<td>
					<button onclick={() => saveFolders(settingsOf().filter((_, j) => j !== i))}>Remove</button>
				</td>
			</tr>
		{/each}
	</tbody>
</table>
<p>
	<button
		onclick={() => saveFolders([...settingsOf(), { models: [], path: null, firmware_version: null }])}
		>Add folder</button
	>
</p>
