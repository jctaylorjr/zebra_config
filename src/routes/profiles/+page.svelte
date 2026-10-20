<script lang="ts">
	import { page } from '$app/state';
	import { beforeNavigate, goto } from '$app/navigation';
	import { invoke } from '@tauri-apps/api/core';
	import { confirm } from '@tauri-apps/plugin-dialog';
	import { onMount } from 'svelte';
	import ZebraFolders from '$lib/ZebraFolders.svelte';
	import { onShortcut } from '$lib/shortcuts';
	import { notify } from '$lib/toast.svelte';
	import type { Media, Profile, Setting } from '$lib/types';

	let saved = $state<Profile[]>([]); // what profiles.json holds
	let draft = $state<Profile[]>([]); // being edited
	let template = $state<Profile | null>(null); // defaults for new profiles and sections
	let index = $state(0);
	let loadError = $state<string | null>(null);
	let error = $state('');
	let note = $state('');
	let preview = $state<Setting[]>([]);
	let previewError = $state('');

	const clone = (v: Profile[]): Profile[] => structuredClone($state.snapshot(v));
	const newMedia = (): Media =>
		structuredClone($state.snapshot(template?.media)) ?? {
			width_in: 2,
			media_type: 'gap',
			sensor: 'transmissive'
		};

	let selected = $derived<Profile | undefined>(draft[index]);
	let dirty = $derived(JSON.stringify(draft) !== JSON.stringify(saved));

	onMount(async () => {
		const list = await invoke<{ profiles: Profile[]; error: string | null }>('list_profiles');
		loadError = list.error;
		saved = list.profiles;
		draft = clone(saved);
		template = await invoke<Profile>('profile_template');
	});

	// the backend works out the settings, so the preview matches what gets sent
	let previewed = 0;
	$effect(() => {
		const p = selected && $state.snapshot(selected);
		const run = ++previewed;
		if (!p) {
			preview = [];
			return;
		}
		invoke<Setting[]>('preview_profile', { profile: p })
			.then((r) => {
				if (run !== previewed) return;
				preview = r;
				previewError = '';
			})
			.catch((e) => {
				if (run !== previewed) return;
				preview = [];
				previewError = String(e);
			});
	});

	// an unsaved profile's id follows its name; a saved one keeps its id, since printers refer to it
	function uniqueId(name: string, self: Profile) {
		const base =
			name
				.toLowerCase()
				.replace(/[^a-z0-9]+/g, '-')
				.replace(/^-|-$/g, '') || 'profile';
		const taken = (id: string) =>
			saved.some((p) => p.id === id) || draft.some((p) => p !== self && p.id === id);
		let id = base;
		for (let n = 2; taken(id); n++) id = `${base}-${n}`;
		return id;
	}

	function rename(p: Profile, name: string) {
		p.name = name;
		if (!saved.some((s) => s.id === p.id)) p.id = uniqueId(name, p);
	}

	function addProfile() {
		if (!template) return;
		const p: Profile = { ...structuredClone($state.snapshot(template)), name: 'New profile' };
		draft.push(p);
		const added = draft[draft.length - 1];
		added.id = uniqueId(added.name, added);
		index = draft.length - 1;
	}

	function deleteProfile() {
		draft.splice(index, 1);
		index = Math.max(0, Math.min(index, draft.length - 1));
	}

	function discard() {
		draft = clone(saved);
		index = Math.min(index, Math.max(0, draft.length - 1));
		error = '';
	}

	async function save() {
		error = '';
		note = '';
		try {
			saved = await invoke<Profile[]>('save_profiles', { profiles: $state.snapshot(draft) });
			draft = clone(saved);
			note = 'Saved';
		} catch (e) {
			error = String(e);
		}
	}

	// exports what's on screen, unsaved edits included
	async function exportProfiles(list: Profile[]) {
		error = '';
		try {
			const path = await invoke<string | null>('export_profiles', {
				profiles: list.map((p) => $state.snapshot(p))
			});
			if (path) notify(`Exported to ${path}`);
		} catch (e) {
			error = String(e);
		}
	}

	// imported profiles join the list unsaved, to look over before saving
	async function importProfiles() {
		error = '';
		let found: Profile[];
		try {
			found = await invoke<Profile[]>('import_profiles');
		} catch (e) {
			error = String(e);
			return;
		}
		if (found.length === 0) return; // cancelled
		let added = 0;
		let replaced = 0;
		let first = -1;
		for (const p of found) {
			const at = draft.findIndex((d) => d.id === p.id);
			if (at >= 0 && JSON.stringify($state.snapshot(draft[at])) === JSON.stringify(p)) continue;
			if (at >= 0) {
				const replace = await confirm(
					`"${draft[at].name}" is already here. Replace it with the imported one, or keep both?`,
					{ title: 'Import profiles', okLabel: 'Replace', cancelLabel: 'Keep both' }
				);
				if (replace) {
					draft[at] = p;
					replaced++;
					if (first < 0) first = at;
					continue;
				}
				p.name = `${p.name} (imported)`;
			}
			draft.push(p);
			const copy = draft[draft.length - 1];
			if (at >= 0) copy.id = uniqueId(copy.name, copy);
			added++;
			if (first < 0) first = draft.length - 1;
		}
		if (first >= 0) index = first;
		const parts = [added && `added ${added}`, replaced && `replaced ${replaced}`].filter(Boolean);
		const summary = parts.length
			? `Imported: ${parts.join(', ')}. Save changes to keep them.`
			: 'Those profiles are already here.';
		notify(summary);
	}

	let leaving = false;
	let tab = $derived(page.url.searchParams.get('tab') === 'folders' ? 'folders' : 'profiles');

	onMount(() =>
		onShortcut('save', () => {
			if (tab === 'profiles' && dirty && !loadError) save();
		})
	);

	// switching tabs keeps the draft, so only leaving the page asks
	beforeNavigate(({ to, cancel }) => {
		if (!dirty || leaving || !to || to.url.pathname === '/profiles') return;
		cancel();
		confirm('Leave without saving your profile changes?', { title: 'Unsaved changes' }).then(
			(ok) => {
				if (!ok) return;
				leaving = true;
				goto(to.url);
			}
		);
	});
</script>

<h1>Profiles</h1>

<div class="tabs" role="tablist">
	<a href="/profiles" role="tab" aria-selected={tab === 'profiles'} class:active={tab === 'profiles'}
		>Profiles</a
	>
	<a
		href="/profiles?tab=folders"
		role="tab"
		aria-selected={tab === 'folders'}
		class:active={tab === 'folders'}>Zebra folders</a
	>
</div>

{#if tab === 'folders'}
	<ZebraFolders />
{:else}
{#if loadError}
	<p class="error">{loadError}. Saving is turned off until the file is fixed or removed.</p>
{/if}

<div class="bar">
	<button class="primary" disabled={!dirty || !!loadError} onclick={save}>Save changes</button>
	<button disabled={!dirty} onclick={discard}>Discard changes</button>
	<span class="gap"></span>
	<button onclick={importProfiles}>Import…</button>
	<button disabled={!selected} onclick={() => selected && exportProfiles([selected])}
		>Export…</button
	>
	<button disabled={draft.length === 0} onclick={() => exportProfiles(draft)}>Export all…</button>
	{#if error}<span class="error">{error}</span>{:else if dirty}<span class="muted"
			>Unsaved changes</span
		>{:else if note}<span class="muted">{note}</span>{/if}
</div>

<div class="columns">
	<aside>
		{#each draft as p, i}
			<button class="item" class:active={i === index} onclick={() => (index = i)}>
				{p.name || 'Untitled'}
				<small>{p.models.length ? p.models.join(', ') : 'any model'}</small>
			</button>
		{/each}
		<button onclick={addProfile} disabled={!template}>New profile</button>
	</aside>

	{#if selected}
		{@const p = selected}
		<section class="editor">
			<label class="field">
				<span>Name</span>
				<input value={p.name} oninput={(e) => rename(p, e.currentTarget.value)} />
			</label>
			<label class="field">
				<span>For models</span>
				<input
					value={p.models.join(', ')}
					placeholder="any model"
					onchange={(e) =>
						(p.models = e.currentTarget.value
							.split(',')
							.map((m) => m.trim())
							.filter(Boolean))}
				/>
				<small class="muted">Parts of the model name, e.g. ZD or ZD621. Blank fits any printer.</small>
			</label>

			<fieldset>
				<legend>
					<label>
						<input
							type="checkbox"
							checked={p.hostname !== null}
							onchange={(e) =>
								(p.hostname = e.currentTarget.checked ? (template?.hostname ?? 'CSC{serial}') : null)}
						/>
						Hostname
					</label>
				</legend>
				{#if p.hostname !== null}
					<input bind:value={p.hostname} />
					<small class="muted"
						>{'{serial}'} becomes the printer's serial number. Sets ip.hostname on Link-OS 7.6 and
						later, and the friendly name on every printer.</small
					>
				{/if}
			</fieldset>

			<fieldset>
				<legend>
					<label>
						<input
							type="checkbox"
							checked={p.darkness !== null}
							onchange={(e) =>
								(p.darkness = e.currentTarget.checked ? (template?.darkness ?? 25) : null)}
						/>
						Darkness
					</label>
				</legend>
				{#if p.darkness !== null}
					<input
						type="number"
						min="0"
						max="30"
						step="0.1"
						value={p.darkness}
						oninput={(e) => {
							const v = e.currentTarget.valueAsNumber;
							if (!Number.isNaN(v)) p.darkness = v;
						}}
					/>
					<small class="muted">0 to 30</small>
				{/if}
			</fieldset>

			<fieldset>
				<legend>
					<label>
						<input
							type="checkbox"
							checked={p.network !== null}
							onchange={(e) =>
								(p.network = e.currentTarget.checked
									? structuredClone($state.snapshot(template?.network)) ?? {
											wired_dhcp: true,
											wireless_dhcp: true,
											primary: 'wired'
										}
									: null)}
						/>
						Network
					</label>
				</legend>
				{#if p.network}
					<label><input type="checkbox" bind:checked={p.network.wired_dhcp} /> Wired uses DHCP</label>
					<label
						><input type="checkbox" bind:checked={p.network.wireless_dhcp} /> Wireless uses DHCP</label
					>
					<label class="field inline">
						<span>Preferred</span>
						<select bind:value={p.network.primary}>
							<option value="wired">Wired</option>
							<option value="wireless">Wireless</option>
						</select>
					</label>
					<small class="muted">An unchecked interface is left as it is.</small>
				{/if}
			</fieldset>

			<fieldset>
				<legend>
					<label>
						<input
							type="checkbox"
							checked={p.media !== null}
							onchange={(e) => (p.media = e.currentTarget.checked ? newMedia() : null)}
						/>
						Media
					</label>
				</legend>
				{#if p.media}
					<label class="field inline">
						<span>Width</span>
						<select
							value={p.media.width_in ?? 0}
							onchange={(e) => p.media && (p.media.width_in = Number(e.currentTarget.value) || null)}
						>
							<option value={0}>Printer's full width</option>
							{#each [1, 2, 3, 4, 5, 6, 8] as w}<option value={w}>{w} in</option>{/each}
						</select>
					</label>
					<label class="field inline">
						<span>Type</span>
						<select bind:value={p.media.media_type}>
							<option value="gap">Gap/notch</option>
							<option value="mark">Black mark</option>
						</select>
					</label>
					<label class="field inline">
						<span>Sensor</span>
						<select bind:value={p.media.sensor}>
							<option value="transmissive">Transmissive</option>
							<option value="reflective">Reflective</option>
						</select>
					</label>
					<small class="muted">Calibrates and prints a test label when media is loaded.</small>
				{/if}
			</fieldset>

			<label class="field">
				<span>Extra commands</span>
				<textarea
					rows="4"
					value={p.commands.join('\n')}
					placeholder="One SGD or ZPL line each, sent last"
					onchange={(e) =>
						(p.commands = e.currentTarget.value
							.split('\n')
							.map((l) => l.trimEnd())
							.filter(Boolean))}
				></textarea>
			</label>

			<button class="danger" onclick={deleteProfile}>Delete profile</button>
		</section>

		<section class="preview">
			<h2>What it sets</h2>
			{#if previewError}
				<p class="error">{previewError}</p>
			{:else if preview.length === 0 && p.commands.length === 0}
				<p class="muted">Nothing yet. Turn on a section to change a setting.</p>
			{:else}
				<table>
					<tbody>
						{#each preview as s}
							<tr>
								<td>{s.label}<br /><code class="muted">{s.key}</code></td>
								<td>{s.value}</td>
							</tr>
						{/each}
						{#each p.commands as c}
							<tr><td>Extra command</td><td><code>{c}</code></td></tr>
						{/each}
					</tbody>
				</table>
			{/if}
		</section>
	{:else}
		<p class="muted">No profiles. Make one with New profile.</p>
	{/if}
</div>
{/if}

<style>
	.tabs {
		display: flex;
		gap: 4px;
		margin-bottom: 14px;
		border-bottom: 1px solid var(--border);
	}
	.tabs a {
		padding: 6px 14px;
		color: var(--muted);
		text-decoration: none;
		border-bottom: 2px solid transparent;
		margin-bottom: -1px;
	}
	.tabs a.active {
		color: var(--accent);
		border-bottom-color: var(--accent);
		font-weight: 600;
	}
	.bar {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 8px;
		margin-bottom: 14px;
	}
	.gap {
		width: 12px;
	}
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
		gap: 12px;
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
	.field.inline {
		flex-direction: row;
		align-items: center;
	}
	.field.inline > span {
		width: 70px;
		font-weight: normal;
	}
	fieldset {
		border: 1px solid var(--border);
		border-radius: 8px;
		padding: 8px 12px 12px;
		display: flex;
		flex-direction: column;
		gap: 6px;
		background: var(--panel);
	}
	legend {
		font-weight: 600;
		padding: 0 4px;
	}
	.danger {
		align-self: flex-start;
		color: var(--danger);
	}
</style>
