export type EngineId = "native" | "electron";

export interface EngineReading {
	ready: number;
	memory: number;
	cpu: number;
}

export interface Workload {
	id: string;
	label: string;
	chats: number;
	messages: number;
	native: EngineReading;
	electron: EngineReading;
}

export const WORKLOADS: Workload[] = [
	{ id: "smoke", label: "2 × 20", chats: 2, messages: 20, native: { ready: 0.6, memory: 375, cpu: 1.1 }, electron: { ready: 1.15, memory: 964, cpu: 8.3 } },
	{ id: "mid1500", label: "1 × 1,500", chats: 1, messages: 1500, native: { ready: 0.6, memory: 384, cpu: 1.3 }, electron: { ready: 1.14, memory: 957, cpu: 6.8 } },
	{ id: "big", label: "1 × 10,000", chats: 1, messages: 10000, native: { ready: 0.61, memory: 419, cpu: 1.2 }, electron: { ready: 1.15, memory: 959, cpu: 6.9 } },
	{ id: "many1500", label: "20 × 1,500", chats: 20, messages: 1500, native: { ready: 0.6, memory: 388, cpu: 1.1 }, electron: { ready: 1.17, memory: 992, cpu: 6.7 } },
	{ id: "many", label: "20 × 5,000", chats: 20, messages: 5000, native: { ready: 0.59, memory: 407, cpu: 0.9 }, electron: { ready: 1.16, memory: 991, cpu: 5.6 } },
];

export const PROCESSES: Record<EngineId, number> = { native: 1, electron: 6 };

export const MEMORY_MAX = 1200;
export const CPU_MAX = 10;

export const TYPING = {
	native: { p50: 35, p95: 51 },
	electron: { p50: 25, p95: 37 },
};

export const SCROLL_1500 = {
	native: { open: 129, longest: 150, slow: 18 },
	electron: { open: 181, longest: 500, slow: 11 },
};

export const IDLE_BUG = {
	skewed: { renders: "62–70", notifications: "62–70", cpu: "28.9–29.6%" },
	normal: { renders: "2", notifications: "0", cpu: "not measured" },
	fixed: { renders: "~2", notifications: "0", cpu: "0.8–1.1%" },
	engineAlone: "0.3%",
};

export const fraction = (value: number, max: number) => Math.max(0, Math.min(1, value / max));

export function strokeRate(cpu: number) {
	return cpu / 2.4;
}

export const seconds = (value: number) => value.toFixed(2) + " s";
export const megabytes = (value: number) => Math.round(value).toLocaleString("en-US") + " MB";
export const percent = (value: number) => value.toFixed(1) + "%";
