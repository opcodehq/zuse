import { createOpenAPI } from "fumadocs-openapi/server";

export const openapi = createOpenAPI({ input: ["./openapi/zuse-api.json"] });
