import { DocsLayout } from "fumadocs-ui/layouts/docs";
import { SearchTrigger } from "fumadocs-ui/layouts/shared/slots/search-trigger";
import { ArrowUpRight, Download } from "lucide-react";
import type { ReactNode } from "react";
import { CleanSearchTrigger } from "@/components/clean-search-trigger";
import { baseOptions, DOWNLOAD_URL, PRODUCT_URL } from "@/lib/layout.shared";
import { documentationTree } from "@/lib/source";

export default function DocumentationLayout({
	children,
}: {
	children: ReactNode;
}) {
	return (
		<DocsLayout
			tree={documentationTree}
			{...baseOptions()}
			tabMode="auto"
			sidebar={{ collapsible: false, footer: <SidebarFooter /> }}
			slots={{
				searchTrigger: {
					sm: SearchTrigger,
					full: CleanSearchTrigger,
				},
			}}
		>
			{children}
		</DocsLayout>
	);
}

function SidebarFooter() {
	return (
		<nav aria-label="Zuse" className="sidebar-footer-links">
			<a href={DOWNLOAD_URL} target="_blank" rel="noreferrer noopener">
				<Download aria-hidden="true" />
				Download Zuse
			</a>
			<a href={PRODUCT_URL} target="_blank" rel="noreferrer noopener">
				<ArrowUpRight aria-hidden="true" />
				Open zuse.sh
			</a>
		</nav>
	);
}
