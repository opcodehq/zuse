export const TITLE = "Fix the flaky retry test";
export const ASK_TEXT = "The retry test in api/client.test.ts fails about one run in five. Find out why and fix it.";
export const COMMAND = "bun test api/client.test.ts";
export const STEER = "Keep retry() public. Put the backoff in a helper.";

export interface DiffRow {
	kind: "hunk" | "context" | "add" | "del";
	text: string;
	old?: number;
	new?: number;
}

export interface FileDiff {
	path: string;
	rows: DiffRow[];
}

function rows(start: [number, number], lines: string[]): DiffRow[] {
	let [old, next] = start;
	return lines.map((line) => {
		if (line.startsWith("@@")) {
			const match = /-(\d+).*\+(\d+)/.exec(line);
			if (match) [old, next] = [Number(match[1]), Number(match[2])];
			return { kind: "hunk", text: line };
		}
		if (line.startsWith("+")) return { kind: "add", text: line.slice(1), new: next++ };
		if (line.startsWith("-")) return { kind: "del", text: line.slice(1), old: old++ };
		return { kind: "context", text: line.slice(1), old: old++, new: next++ };
	});
}

export const DIFF: FileDiff[] = [
	{
		path: "api/client.ts",
		rows: rows(
			[1, 1],
			[
				"@@ -18,16 +18,20 @@ export async function retry<T>(",
				" export async function retry<T>(run: () => Promise<T>, attempts = 3): Promise<T> {",
				"-  let last: unknown;",
				"-  for (let i = 0; i < attempts; i++) {",
				"-    try {",
				"-      return await run();",
				"-    } catch (error) {",
				"-      last = error;",
				"-      await sleep(100);",
				"-    }",
				"-  }",
				"-  throw last;",
				"+  return withBackoff(run, attempts, { base: 100, cap: 2_000 });",
				"+}",
				"+",
				"+async function withBackoff<T>(run: () => Promise<T>, attempts: number, delay: Backoff): Promise<T> {",
				"+  for (let attempt = 1; ; attempt++) {",
				"+    try {",
				"+      return await run();",
				"+    } catch (error) {",
				"+      if (attempt >= attempts || !isRetryable(error)) throw error;",
				"+      await sleep(Math.min(delay.cap, delay.base * 2 ** (attempt - 1)));",
				"+    }",
				"+  }",
				"+}",
				"+",
				" }",
			],
		),
	},
	{
		path: "api/client.test.ts",
		rows: rows(
			[1, 1],
			[
				"@@ -41,9 +41,13 @@ describe(\"retry\", () => {",
				"   it(\"gives up after three attempts\", async () => {",
				"-    const started = Date.now();",
				"+    vi.useFakeTimers();",
				"     const run = vi.fn().mockRejectedValue(new TimeoutError());",
				"-    await expect(retry(run)).rejects.toThrow(TimeoutError);",
				"-    expect(Date.now() - started).toBeLessThan(250);",
				"+    const result = retry(run);",
				"+    await vi.runAllTimersAsync();",
				"+    await expect(result).rejects.toThrow(TimeoutError);",
				"+    expect(run).toHaveBeenCalledTimes(3);",
				"+    vi.useRealTimers();",
				"   });",
			],
		),
	},
];

export const count = (file: FileDiff, kind: "add" | "del") => file.rows.filter((row) => row.kind === kind).length;
