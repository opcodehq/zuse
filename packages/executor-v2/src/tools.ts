export const PLUGIN_MCP_TOOLS = [
	{
		name: "plugins_list",
		description:
			"List enabled, connected Zuse plugins and account labels. Optional: search directly when you already know the service or task.",
		inputSchema: {
			type: "object",
			properties: {},
			required: [],
			additionalProperties: false,
		},
	},
	{
		name: "plugins_search",
		description:
			"Search the current workspace's connected Zuse plugins directly by service and task, for example 'linear issue'. No listing or empty query is needed first. Returns exact tool addresses; use plugins_schema then plugins_call. An empty result means no matching tools; use plugins_list to check connected services before concluding none are connected.",
		inputSchema: {
			type: "object",
			properties: { query: { type: "string" } },
			required: ["query"],
			additionalProperties: false,
		},
	},
	{
		name: "plugins_schema",
		description:
			"Get the input schema for a connected plugin tool by its exact address.",
		inputSchema: {
			type: "object",
			properties: { address: { type: "string" } },
			required: ["address"],
			additionalProperties: false,
		},
	},
	{
		name: "plugins_call",
		description:
			"Call a connected plugin tool using its schema. May read or modify external data; subject to session permission policy.",
		inputSchema: {
			type: "object",
			properties: {
				address: { type: "string" },
				arguments: { type: "object" },
			},
			required: ["address", "arguments"],
			additionalProperties: false,
		},
	},
];
