import { copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseGenerativeUiSpec } from "../../../packages/utils/src/generative-ui.ts";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const output = join(root, "app/src/main/assets/native");
await mkdir(output, { recursive: true });
const result = await Bun.build({
	entrypoints: [join(root, "scripts/ui-parser.ts")],
	outdir: output,
	naming: "UIParser.js",
	target: "browser",
	format: "iife",
	minify: true,
});
if (!result.success)
	throw new AggregateError(result.logs, "Native UI parser build failed.");
const rendererRequire = createRequire(join(root, "../renderer/package.json"));
const mermaid = rendererRequire.resolve("mermaid/dist/mermaid.min.js");
await copyFile(mermaid, join(output, "Mermaid.js"));
await copyFile(
	join(dirname(dirname(mermaid)), "LICENSE"),
	join(output, "Mermaid-LICENSE.txt"),
);
const provenance = {
	parserSource: "packages/utils/src/generative-ui.ts",
	mermaidVersion: JSON.parse(
		await readFile(join(dirname(dirname(mermaid)), "package.json"), "utf8"),
	).version,
};
await writeFile(
	join(output, "provenance.json"),
	`${JSON.stringify(provenance, null, "\t")}\n`,
);

const repositoryRequire = createRequire(join(root, "../../package.json"));
const notices = await Promise.all(
	[
		[
			"OpenUI Lang Core",
			join(
				dirname(dirname(repositoryRequire.resolve("@openuidev/lang-core"))),
				"LICENSE",
			),
		],
		["Zod", join(dirname(repositoryRequire.resolve("zod")), "LICENSE")],
		["Mermaid", join(dirname(dirname(mermaid)), "LICENSE")],
	].map(async ([name, path]) => `${name}\n${await readFile(path, "utf8")}`),
);
await writeFile(join(output, "ThirdPartyNotices.txt"), notices.join("\n\n"));
const spec = `root = Card([metrics, chart, steps, form], "Workspace health")
metrics = Grid([Stat("Connected devices", "3"), Stat("Response time", "42 ms")])
chart = BarChart("Build duration", [{label: "Before", value: 80}, {label: "After", value: 35}], "seconds")
steps = Steps([{label: "Connect the workspace", status: "done"}, {label: "Review changes", status: "active"}])
form = Form([Input("branch", "Branch", "", "feature/native-android"), Checkbox("tests", "Run checks", true)], "Run workspace checks")`;
const previews = join(root, "app/src/debug/assets/native");
await mkdir(previews, { recursive: true });
await writeFile(
	join(previews, "visual-preview.json"),
	JSON.stringify({ spec, root: parseGenerativeUiSpec(spec) }),
);
