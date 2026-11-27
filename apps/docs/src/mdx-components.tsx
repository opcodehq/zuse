import defaultMdxComponents from "fumadocs-ui/mdx";
import type { MDXComponents } from "mdx/types";
import type { ComponentProps } from "react";
import { mdxComponents } from "@/components/mdx-components";

function Table(props: ComponentProps<"table">) {
	return (
		<div
			className="relative overflow-auto prose-no-margin my-6"
			tabIndex={0}
		>
			<table {...props} />
		</div>
	);
}

export function getMDXComponents(components?: MDXComponents): MDXComponents {
	return {
		...defaultMdxComponents,
		table: Table,
		...mdxComponents,
		...components,
	};
}
