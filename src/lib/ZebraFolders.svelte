<script lang="ts">
	import { invoke } from '@tauri-apps/api/core';
	import { open } from '@tauri-apps/plugin-dialog';
	import { onMount } from 'svelte';
	import { notify } from './toast.svelte';
	import type { FolderSetting, FolderView } from './types';

	// each change saves straight away, unlike profiles: there's no draft to look over
	let folders = $state<FolderView[]>([]);
	let index = $state(0);
	let error = $state('');

	let selected = $derived<FolderView | undefined>(folders[index]);
	let fromFile = $derived(selected?.summary?.installs ?? null);

	onMount(async () => {
		folders = await invoke<FolderView[]>('list_folders');
	});

	const settingsOf = () =>
		folders.map(({ models, path, firmware_version }) => ({ models, path, firmware_version }));

	async function save(next: FolderSetting[], done: string) {
		error = '';
		try {
			folders = await invoke<FolderView[]>('save_folders', { folders: next });
			notify(done);
		} catch (e) {
			error = String(e);
			folders = await invoke<FolderView[]>('list_folders'); // undo the edit in the inputs
		}
		index = Math.max(0, Math.min(index, folders.length - 1));
	}

	function change(edit: Partial<FolderSetting>) {
		const next = settingsOf();
		next[index] = { ...next[index], ...edit };
		save(next, 'Saved');
	}

	async function choose() {
		const path = await open({
			directory: true,
			title: 'Choose the Zebra folder',
			defaultPath: selected?.path ?? undefined
		});
		if (typeof path === 'string') change({ path });
	}

	async function add() {
		await save([...settingsOf(), { models: [], path: null, firmware_version: null }], 'Added a folder');
		index = folders.length - 1;
	}

	function remove() {
		save(
			settingsOf().filter((_, j) => j !== index),
			'Removed the folder'
		);
	}

	// every folder is called Zebra, so name it by what it's in, e.g. ZEBRA_USB
	const place = (path: string | null) => path?.split(/[\\/]/).filter(Boolean).at(-2) ?? null;
</script>

<p class="muted">
	A folder laid out like a USB mirror stick (appl, files, commands). Each printer gets the first
	folder whose models match it, so firmware only goes to the models it's meant for. Changes save
	straight away.
</p>
{#if error}<p class="error">{error}</p>{/if}

<div class="columns">
	<aside>
		{#each folders as f, i}
			<button class="item" class:active={i === index} onclick={() => (index = i)}>
				{f.models.length ? f.models.join(', ') : 'No models yet'}
				<small>{place(f.path) ?? 'no folder chosen'}</small>
			</button>
		{/each}
		<button onclick={add}>Add folder</button>
	</aside>

	{#if selected}
		<section class="editor">
			<label class="field">
				<span>For models</span>
				<input
					value={selected.models.join(', ')}
					placeholder="e.g. ZD611, ZD621"
					onchange={(e) =>
						change({
							models: e.currentTarget.value
								.split(',')
								.map((m) => m.trim())
								.filter(Boolean)
						})}
				/>
				<small class="muted">Parts of the model name. A folder with no models is never used.</small>
			</label>

			<div class="field">
				<span>Folder</span>
				{#if selected.path}<code>{selected.path}</code>{:else}<span class="muted">none chosen</span>{/if}
				<div class="buttons">
					<button onclick={choose}>Choose…</button>
					{#if selected.path}<button onclick={() => change({ path: null })}>Clear</button>{/if}
				</div>
			</div>

			<label class="field">
				<span>Firmware version</span>
				<input
					value={selected.firmware_version ?? ''}
					placeholder={fromFile ? `${fromFile}, from the file name` : 'blank = the file name'}
					onchange={(e) => change({ firmware_version: e.currentTarget.value.trim() || null })}
				/>
				<small class="muted"
					>The appl.name its firmware installs. Printers already on it skip the firmware. Blank
					goes by the firmware file's name, as a printer reading a USB mirror does.</small
				>
			</label>

			<button class="danger" onclick={remove}>Remove folder</button>
		</section>

		<section class="preview">
			<h2>What it sends</h2>
			{#if !selected.path}
				<p class="muted">Choose a folder to see what's in it.</p>
			{:else if selected.summary?.error}
				<p class="error">{selected.summary.error}</p>
			{:else if selected.summary}
				<table>
					<tbody>
						<tr>
							<td>Firmware</td>
							<td>{selected.summary.firmware ?? 'none'}</td>
						</tr>
						<tr><td>Files for E:</td><td>{selected.summary.files}</td></tr>
						<tr><td>Command files</td><td>{selected.summary.commands}</td></tr>
						<tr>
							<td>Checked on E: afterwards</td>
							<td>{selected.summary.checks.length ? selected.summary.checks.join(', ') : 'nothing'}</td>
						</tr>
					</tbody>
				</table>
			{/if}
		</section>
	{/if}
</div>

<style>
	.columns {
		display: flex;
		flex-wrap: wrap;
		gap: 18px;
		align-items: flex-start;
	}
	aside {
		width: 190px;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.item {
		text-align: left;
		display: flex;
		flex-direction: column;
	}
	.item small {
		color: var(--muted);
	}
	.item.active {
		border-color: var(--accent);
		background: color-mix(in srgb, var(--accent) 12%, var(--panel));
	}
	.editor {
		flex: 1 1 340px;
		max-width: 440px;
		display: flex;
		flex-direction: column;
		gap: 14px;
	}
	.preview {
		flex: 1 1 320px;
	}
	.preview h2 {
		margin-top: 0;
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}
	.field > span {
		font-weight: 600;
	}
	.field code {
		overflow-wrap: anywhere;
	}
	.buttons {
		display: flex;
		gap: 6px;
	}
	.danger {
		align-self: flex-start;
		color: var(--danger);
	}
</style>
