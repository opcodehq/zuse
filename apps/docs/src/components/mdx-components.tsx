import Link from "fumadocs-core/link";
import { Callout as FumadocsCallout } from "fumadocs-ui/components/callout";
import { DynamicCodeBlock } from "fumadocs-ui/components/dynamic-codeblock";
import {
	Step as FumadocsStep,
	Steps as FumadocsSteps,
} from "fumadocs-ui/components/steps";
import Image from "next/image";
import type { ReactNode } from "react";
import { FEATURES, SURFACES, type Surface } from "../data/features";

export type PageLink = { href: string; title: string };

export function Callout({
	title = "Good to know",
	children,
}: {
	title?: string;
	children: ReactNode;
}) {
	return <FumadocsCallout title={title}>{children}</FumadocsCallout>;
}

export function Warning({
	title = "Before you continue",
	children,
}: {
	title?: string;
	children: ReactNode;
}) {
	return (
		<FumadocsCallout type="warn" title={title}>
			{children}
		</FumadocsCallout>
	);
}

export function SecurityNote({
	title = "Security",
	children,
}: {
	title?: string;
	children: ReactNode;
}) {
	return (
		<FumadocsCallout type="idea" title={title}>
			{children}
		</FumadocsCallout>
	);
}

export function Troubleshooting({
	title = "If this does not work",
	children,
}: {
	title?: string;
	children: ReactNode;
}) {
	return (
		<FumadocsCallout type="warning" title={title}>
			{children}
		</FumadocsCallout>
	);
}

export function Command({ children }: { children: ReactNode }) {
	return (
		<DynamicCodeBlock
			lang="bash"
			code={String(children)}
			codeblock={{ title: "Terminal" }}
		/>
	);
}

export function GuideHero({
	label,
	children,
}: {
	label?: string;
	children: ReactNode;
}) {
	return (
		<section className="guide-hero">
			{label ? <span>{label}</span> : null}
			<div>{children}</div>
		</section>
	);
}

export function Workflow({ children }: { children: ReactNode }) {
	return <FumadocsSteps>{children}</FumadocsSteps>;
}

export function Step({
	title,
	children,
}: {
	title: string;
	children: ReactNode;
}) {
	return (
		<FumadocsStep>
			<h3>{title}</h3>
			{children}
		</FumadocsStep>
	);
}

export function ProductFrame({
	title = "Zuse",
	children,
}: {
	title?: string;
	children: ReactNode;
}) {
	return (
		<figure className="product-frame">
			<figcaption>
				<span className="product-frame-dot"></span>
				{title}
			</figcaption>
			<div>{children}</div>
		</figure>
	);
}

const ARTWORK = {
	observatory: "/brand/observatory-dither.webp",
	pegasus: "/brand/pegasus-dither.webp",
	handoff: "/brand/handoff-dither.webp",
} as const;

export function Screenshot({
	src,
	alt,
	title,
	caption,
	art = "observatory",
	width = 2400,
	height = 1500,
	flush = false,
	priority = false,
	sizes = "(min-width: 1024px) 56rem, 100vw",
}: {
	src: string;
	alt: string;
	title?: string;
	caption?: string;
	art?: keyof typeof ARTWORK;
	width?: number;
	height?: number;
	flush?: boolean;
	priority?: boolean;
	sizes?: string;
}) {
	return (
		<figure className="docs-screenshot not-prose">
			<div
				className="docs-screenshot-stage"
				data-flush={flush ? "" : undefined}
				style={{ backgroundImage: `url(${ARTWORK[art]})` }}
			>
				<Image
					src={src}
					alt={alt}
					width={width}
					height={height}
					priority={priority}
					sizes={sizes}
				/>
			</div>
			{title || caption ? (
				<figcaption>
					{title ? <strong>{title}</strong> : null}
					{caption}
				</figcaption>
			) : null}
		</figure>
	);
}

/** A built-in provider mark from `public/logos`. */
export function ProviderLogo({ name }: { name: string }) {
	return <img src={`/logos/${name}.svg`} alt="" data-logo-icon="" />;
}

/** A monochrome ACP registry mark from `public/logos/acp`, tinted like text. */
export function AgentLogo({ id }: { id: string }) {
	return (
		<span
			aria-hidden="true"
			className="agent-logo"
			style={{ maskImage: `url(/logos/acp/${id}.svg)` }}
		/>
	);
}

export function FeatureGrid({ children }: { children: ReactNode }) {
	return <div className="feature-grid not-prose">{children}</div>;
}

export function Feature({
	href,
	...screenshot
}: Parameters<typeof Screenshot>[0] & { href: string }) {
	return (
		<Link href={href}>
			<Screenshot sizes="(min-width: 768px) 28rem, 100vw" {...screenshot} />
		</Link>
	);
}

export function Hotspot({
	label,
	children,
}: {
	label: string;
	children: ReactNode;
}) {
	return (
		<span className="hotspot">
			<span>{label}</span>
			{children}
		</span>
	);
}

export function CommandTabs({ children }: { children: ReactNode }) {
	return <div className="command-tabs">{children}</div>;
}

export function CodeDiff({ children }: { children: ReactNode }) {
	return <div className="code-diff">{children}</div>;
}

export function FileTree({ children }: { children: ReactNode }) {
	return <div className="file-tree">{children}</div>;
}

export function SchemaTable({ children }: { children: ReactNode }) {
	return <div className="schema-table">{children}</div>;
}

export function PlatformBadge({ children }: { children: ReactNode }) {
	return <span className="meta-chip">{children}</span>;
}

export function ProviderBadge({ children }: { children: ReactNode }) {
	return <span className="meta-chip meta-chip-provider">{children}</span>;
}

export function Availability({
	state = "supported",
}: {
	state?: "supported" | "partial" | "host";
}) {
	return (
		<span className={`availability availability-${state}`}>
			{state === "host"
				? "Host"
				: state === "partial"
					? "Partial"
					: "Supported"}
		</span>
	);
}

export function CapabilityMatrix({
	features,
}: {
	features?: ReadonlyArray<keyof typeof FEATURES>;
}) {
	const rows = (features ?? Object.keys(FEATURES)) as ReadonlyArray<
		keyof typeof FEATURES
	>;
	return (
		<div className="capability-table-wrap">
			<table className="capability-table">
				<thead>
					<tr>
						<th>Capability</th>
						{SURFACES.map((surface) => (
							<th key={surface}>{surface.replace("-", " ")}</th>
						))}
					</tr>
				</thead>
				<tbody>
					{rows.map((key) => {
						const feature = FEATURES[key];
						return (
							<tr key={key}>
								<th>
									<strong>{feature.title}</strong>
									<span>{feature.description}</span>
								</th>
								{SURFACES.map((surface) => {
									const state = feature.surfaces[surface as Surface];
									return (
										<td key={surface}>
											{state ? (
												<Availability state={state} />
											) : (
												<span title="Not available">—</span>
											)}
										</td>
									);
								})}
							</tr>
						);
					})}
				</tbody>
			</table>
		</div>
	);
}

export function ArchitectureDiagram({ children }: { children: ReactNode }) {
	return <figure className="architecture-diagram">{children}</figure>;
}

function LinkPanel({
	label,
	links,
}: {
	label: string;
	links: ReadonlyArray<PageLink>;
}) {
	if (links.length === 0) return null;
	return (
		<aside className="link-panel">
			<span>{label}</span>
			<ul>
				{links.map((link) => (
					<li key={link.href}>
						<Link href={link.href}>{link.title}</Link>
					</li>
				))}
			</ul>
		</aside>
	);
}

export function Prerequisites({ links }: { links: ReadonlyArray<PageLink> }) {
	return <LinkPanel label="Before you start" links={links} />;
}

export function RelatedPages({ links }: { links: ReadonlyArray<PageLink> }) {
	return <LinkPanel label="Related" links={links} />;
}

export function NextStep({ href, label }: { href: string; label: string }) {
	return (
		<a className="next-step" href={href}>
			<span>Continue</span>
			<strong>{label}</strong>
			<span aria-hidden="true">→</span>
		</a>
	);
}

export const mdxComponents = {
	GuideHero,
	Workflow,
	Step,
	ProductFrame,
	Screenshot,
	FeatureGrid,
	ProviderLogo,
	AgentLogo,
	Feature,
	Hotspot,
	Command,
	CommandTabs,
	CodeDiff,
	FileTree,
	SchemaTable,
	CapabilityMatrix,
	PlatformBadge,
	ProviderBadge,
	Availability,
	Callout,
	Warning,
	SecurityNote,
	Troubleshooting,
	ArchitectureDiagram,
	NextStep,
};
