import type { Details, FolderView, Printer, PrinterSetting, Profile } from './types';

// what the Printer page has read and done for one printer
export type PrinterView = {
	details: Details | null;
	detailsError: string;
	updated: string;
	all: PrinterSetting[] | null; // every setting it reports
	allError: string;
	allLoading: boolean;
	filter: string;
	busy: string | null; // the command running
	actionNote: string;
	actionError: string;
	resetNote: string;
	resetError: string;
	rawCommand: string;
	rawReply: string | null;
	rawBusy: boolean;
	calMethod: string; // empty until picked: follows the printer's media type
	calBusy: boolean;
	calNote: string;
	calError: string;
	setupError: string;
	editing: string | null; // the setting being changed: print width, darkness or speed
	editValue: string;
	editBusy: boolean;
	editNote: string;
	editError: string;
	unrecorded: boolean; // opened, but not yet added to the recent printers
	autoRefresh: boolean | null; // null until changed: on, except for battery printers
};

// the Printer page, kept while visiting other pages
export const printerPage = $state({
	ip: '', // the printer opened last; the sidebar link goes back to it
	typed: '',
	printers: [] as Printer[],
	profiles: [] as Profile[],
	folders: [] as FolderView[],
	views: {} as Record<string, PrinterView>
});

// a printer's view, made the first time it's opened. it changes state, so it's for effects
// and event handlers, not for rendering
export function viewFor(ip: string): PrinterView {
	printerPage.views[ip] ??= {
		details: null,
		detailsError: '',
		updated: '',
		all: null,
		allError: '',
		allLoading: false,
		filter: '',
		busy: null,
		actionNote: '',
		actionError: '',
		resetNote: '',
		resetError: '',
		rawCommand: '',
		rawReply: null,
		rawBusy: false,
		calMethod: '',
		calBusy: false,
		calNote: '',
		calError: '',
		setupError: '',
		editing: null,
		editValue: '',
		editBusy: false,
		editNote: '',
		editError: '',
		unrecorded: true,
		autoRefresh: null
	};
	return printerPage.views[ip];
}
