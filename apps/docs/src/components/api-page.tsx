"use client";

import { createOpenAPIPage } from "fumadocs-openapi/ui";
import { ATTACHMENT_MEDIA_TYPES } from "@/lib/attachment-media-types";

/** Attachment uploads send the file's raw bytes as the request body. */
const rawBody = {
	encode: ({ body }: { body: unknown }) => body as BodyInit,
	generateExample: () => undefined,
};

export const APIPage = createOpenAPIPage({
	playground: { enabled: false },
	mediaAdapters: Object.fromEntries(
		ATTACHMENT_MEDIA_TYPES.filter((type) => type !== "application/json").map(
			(type) => [type, rawBody],
		),
	),
	shikiOptions: {
		themes: { light: "github-light", dark: "github-dark" },
	},
});
