// shapes the Rust side sends; see src-tauri/src/model.rs

export type Status =
	| { state: 'Pending' }
	| { state: 'Applying' | 'Applied' | 'Unverified' | 'Failed'; detail: string }
	| { state: 'Offline'; detail: { attempt: number; retry_in_secs: number; reason: string } };

export type Printer = {
	ip: string;
	model: string | null;
	serial: string | null;
	firmware: string | null;
	profile_id: string | null;
	status: Status;
};

export type Network = {
	wired_dhcp: boolean;
	wireless_dhcp: boolean;
	primary: 'wired' | 'wireless';
};

export type Media = {
	width_in: number | null;
	media_type: 'gap' | 'mark';
	sensor: 'transmissive' | 'reflective';
};

export type Profile = {
	id: string;
	name: string;
	models: string[];
	hostname: string | null;
	darkness: number | null;
	network: Network | null;
	media: Media | null;
	commands: string[];
};

export type Setting = { label: string; key: string; value: string };

export type FolderSetting = {
	models: string[];
	path: string | null;
	firmware_version: string | null;
};

export type FolderView = FolderSetting & {
	summary: {
		firmware: string | null;
		files: number;
		commands: number;
		checks: string[];
		error: string | null;
	} | null;
};

// same rules as model::fits and Settings::folder_for
export const fits = (models: string[], model: string | null) =>
	models.length === 0 || models.some((m) => model?.includes(m));

export const folderFor = (folders: FolderView[], model: string | null) =>
	folders.find((f) => f.path && f.models.length > 0 && fits(f.models, model));

export const statusText = (s: Status) => {
	switch (s.state) {
		case 'Offline':
			return `Offline (${s.detail.reason}): attempt ${s.detail.attempt}, retry in ${s.detail.retry_in_secs}s`;
		case 'Failed':
			return `Failed: ${s.detail}`;
		case 'Applied':
			return `Applied, checked ${s.detail}`;
		case 'Unverified':
			return `Sent, not verified: ${s.detail}`;
		case 'Applying':
			return `${s.detail}…`;
		default:
			return s.state;
	}
};

// the printer page's details; see printer_details in src-tauri/src/lib.rs
export type Details = {
	values: Record<string, unknown>; // settings the printer doesn't have are null
	status: string;
	media: boolean;
	head_closed: boolean;
	paused: boolean;
	errors: string[];
	warnings: string[];
};

export type PrinterSetting = { key: string; value: string; choices: string | null };

export const printerHref = (ip: string) => `/printer?ip=${encodeURIComponent(ip)}`;
