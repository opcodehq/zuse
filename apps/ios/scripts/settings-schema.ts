import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { Schema } from "effect";
import {
	CloudAuthConfigureRequest,
	CloudAuthLoginStartRequest,
} from "../../../packages/contracts/src/cloud-auth.ts";
import { CloudProviderConnectionInput } from "../../../packages/contracts/src/cloud-workspaces.ts";
import {
	SettingsPatch,
	WorkspaceSettingsValues,
} from "../../../packages/contracts/src/settings.ts";

// The native settings editor derives its fields and choices from the RPC contract.
const document = Schema.toJsonSchemaDocument(SettingsPatch);
const cloud = Schema.toJsonSchemaDocument(CloudProviderConnectionInput);
const workspace = Schema.toJsonSchemaDocument(WorkspaceSettingsValues);
const auth = Schema.toJsonSchemaDocument(CloudAuthConfigureRequest);
const login = Schema.toJsonSchemaDocument(CloudAuthLoginStartRequest);
const content = `${JSON.stringify(
	{
		...document.schema,
		$defs: document.definitions,
		cloudProviderConnection: { ...cloud.schema, $defs: cloud.definitions },
		workspaceSettings: { ...workspace.schema, $defs: workspace.definitions },
		cloudAuthConfigure: { ...auth.schema, $defs: auth.definitions },
		cloudAuthLogin: { ...login.schema, $defs: login.definitions },
	},
	null,
	2,
)}\n`;
const destination = resolve(import.meta.dir, "../App/SettingsSchema.json");
if (process.argv.includes("--check")) {
	if (readFileSync(destination, "utf8") !== content) {
		throw new Error(
			"Native settings schema is stale; run bun apps/ios/scripts/settings-schema.ts",
		);
	}
} else {
	writeFileSync(destination, content);
}
