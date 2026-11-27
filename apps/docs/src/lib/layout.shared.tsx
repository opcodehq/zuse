import type { BaseLayoutProps } from "fumadocs-ui/layouts/shared";
import Image from "next/image";

export const SITE_URL = "https://docs.zuse.sh";
export const GITHUB_URL = "https://github.com/opcodehq/zuse";
export const PRODUCT_URL = "https://zuse.sh";
export const DOWNLOAD_URL = "https://zuse.sh/download";
const X_URL = "https://x.com/zuse_sh";
const DISCORD_URL = "https://discord.gg/9E8Rhd3ke";

export function baseOptions(): BaseLayoutProps {
	return {
		nav: {
			url: "/start",
			title: (
				<span className="docs-wordmark">
					<Image
						src="/app-icon.png"
						width="22"
						height="22"
						alt=""
						priority
						className="size-[22px] rounded-[6px]"
					/>
					<span>Zuse</span>
				</span>
			),
			transparentMode: "none",
		},
		links: [
			{
				type: "icon",
				label: "Zuse on X",
				text: "X",
				url: X_URL,
				external: true,
				icon: <XIcon />,
			},
			{
				type: "icon",
				label: "Zuse Discord",
				text: "Discord",
				url: DISCORD_URL,
				external: true,
				icon: <DiscordIcon />,
			},
			{
				type: "icon",
				label: "Zuse on GitHub",
				text: "GitHub",
				url: GITHUB_URL,
				external: true,
				icon: <GitHubIcon />,
			},
		],
	};
}

function XIcon() {
	return (
		<svg aria-hidden="true" viewBox="0 0 24 24" fill="currentColor">
			<path d="M17.75 3h3.07l-6.7 7.66L22 21h-6.17l-4.83-6.32L5.47 21H2.4l7.17-8.2L2 3h6.33l4.37 5.77L17.75 3Zm-1.08 16.18h1.7L7.4 4.73H5.58l11.09 14.45Z" />
		</svg>
	);
}

function DiscordIcon() {
	return (
		<svg aria-hidden="true" viewBox="0 0 24 24" fill="currentColor">
			<path d="M19.6 5.3A17.6 17.6 0 0 0 15.3 4l-.55 1.1a16.3 16.3 0 0 0-5.5 0L8.7 4a17.5 17.5 0 0 0-4.36 1.33C1.6 9.43.85 13.43 1.22 17.37A17.7 17.7 0 0 0 6.57 20l1.15-1.85a11.4 11.4 0 0 1-1.8-.87l.44-.34a12.6 12.6 0 0 0 11.28 0l.44.34c-.57.34-1.18.63-1.8.87L17.43 20a17.6 17.6 0 0 0 5.35-2.63c.44-4.57-.75-8.53-3.18-12.07ZM8.52 14.95c-1.06 0-1.94-.98-1.94-2.18s.85-2.19 1.94-2.19c1.08 0 1.96.99 1.94 2.19 0 1.2-.86 2.18-1.94 2.18Zm6.96 0c-1.07 0-1.94-.98-1.94-2.18s.85-2.19 1.94-2.19c1.08 0 1.96.99 1.94 2.19 0 1.2-.86 2.18-1.94 2.18Z" />
		</svg>
	);
}

function GitHubIcon() {
	return (
		<svg aria-hidden="true" viewBox="0 0 24 24" fill="currentColor">
			<path d="M12 .7a11.5 11.5 0 0 0-3.64 22.41c.58.1.79-.25.79-.56v-2.23c-3.22.7-3.9-1.37-3.9-1.37-.52-1.34-1.28-1.7-1.28-1.7-1.05-.72.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.77 2.7 1.26 3.36.96.1-.75.4-1.26.74-1.55-2.57-.3-5.27-1.29-5.27-5.69 0-1.26.45-2.28 1.18-3.09-.12-.29-.51-1.47.11-3.05 0 0 .96-.31 3.16 1.18a10.95 10.95 0 0 1 5.76 0c2.2-1.49 3.16-1.18 3.16-1.18.62 1.58.23 2.76.11 3.05.74.81 1.18 1.83 1.18 3.09 0 4.41-2.7 5.39-5.28 5.68.42.36.79 1.06.79 2.13v3.16c0 .31.21.67.8.56A11.5 11.5 0 0 0 12 .7Z" />
		</svg>
	);
}
