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
	rawCommand: string;
	rawReply: string | null;
	rawBusy: boolean;
	calMethod: string; // empty until picked: follows the printer's media type
	calBusy: boolean;
	calNote: string;
	calError: string;
	setupError: string;
};

// the Printer page, kept while visiting other pages
export const printerPage = $state({
	ip: '', // the printer opened last; the sidebar link goes back to it
	typed: '',
	autoRefresh: true,
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
		rawCommand: '',
		rawReply: null,
		rawBusy: false,
		calMethod: '',
		calBusy: false,
		calNote: '',
		calError: '',
		setupError: ''
	};
	return printerPage.views[ip];
}
