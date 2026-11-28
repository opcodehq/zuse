import { CalendarDays, Check } from "lucide-react";
import { Button } from "@/components/button";
import { PageMasthead } from "@/components/page-masthead";
import { getSEO } from "@/lib/seo";

export const metadata = getSEO({
	title: "Pricing: local and cloud coding agents",
	description:
		"Zuse is free on your own computer. Zuse Cloud is $1 for the first week, then $20/month with 33,600 machine minutes, or $50/month for Cloud Max. Bring your own agent subscriptions.",
	path: "/pricing",
});

const plans = [
	{
		name: "Free",
		audience: "For your own computer",
		price: "$0",
		period: "during beta",
		features: [
			"Agents on your own computer",
			"Your own subscriptions and keys",
			"Parallel chats and worktrees",
			"Local files, terminal, and diff review",
			"Open-source desktop app",
		],
		cta: "Download free",
		note: "macOS and Linux",
		featured: false,
	},
	{
		name: "Cloud",
		audience: "For getting started",
		price: "$1",
		period: "first week, then $20/mo",
		features: [
			"8,400 machine minutes in the first week",
			"Then 33,600 machine minutes a month (about 140 hours)",
			"Standard computers, 4 vCPU / 16 GB",
			"Pay as you go after, at the same rate",
			"$25 default monthly cap you control",
		],
		cta: "Start for $1",
		note: "Subscribe in the app after you download",
		featured: true,
	},
	{
		name: "Cloud Max",
		audience: "For heavy, parallel use",
		price: "$50",
		period: "/ month",
		features: [
			"84,000 machine minutes a month (about 350 hours)",
			"Standard and Large computers, up to 8 vCPU / 32 GB",
			"Pay as you go after, at the same rate",
			"$25 default monthly cap you control",
		],
		cta: "Get Cloud Max",
		note: "Subscribe in the app after you download",
		featured: false,
	},
];

const costs = [
	{
		title: "$0 for AI tokens",
		body: "Use your own Claude, ChatGPT, and other subscriptions or API keys. Machine minutes never cover model tokens.",
	},
	{
		title: "0% markup",
		body: "Minutes past your allowance cost the same as included ones: $20 per 33,600.",
	},
	{
		title: "Only awake time counts",
		body: "A computer sleeps after about 10 quiet minutes and wakes when you send a message. Asleep computers use no minutes.",
	},
];

const columns = ["Free", "Cloud", "Cloud Max", "Enterprise"];

const comparison = [
	["Machine minutes a month", "—", "33,600", "84,000", "More"],
	["First week for $1", "—", "Yes", "—", "—"],
	["Largest computer", "Your own", "4 vCPU / 16 GB", "8 vCPU / 32 GB", "Larger"],
	["Pay as you go", "—", "Yes", "Yes", "Yes"],
	["Invoicing", "—", "—", "—", "Yes"],
];

export default function PricingPage() {
	return (
		<main>
			<PageMasthead
				eyebrow="Pricing / Bring your own agents"
				title={
					<>
						Start here.
						<br />
						<span className="heading-accent text-primary">Go further.</span>
					</>
				}
				description="Free on your own computer. Cloud plans include machine minutes every month, and the first week of Cloud is $1."
				artwork="handoff"
			/>
			<div className="grid md:grid-cols-3">
				{plans.map((plan) => (
					<section
						key={plan.name}
						className={
							plan.featured
								? "pricing-card flex flex-col bg-primary/5"
								: "pricing-card flex flex-col"
						}
					>
						<div className="flex min-h-6 items-center justify-between gap-3">
							<p className="editorial-label">{plan.audience}</p>
							{plan.featured ? (
								<span className="rounded-full bg-primary px-2.5 py-1 font-mono text-[10px] font-semibold uppercase tracking-wide text-primary-foreground">
									$1 first week
								</span>
							) : null}
						</div>
						<h2 className="mt-4 text-3xl">{plan.name}</h2>
						<p
							className={
								plan.featured
									? "mt-6 font-mono text-5xl tracking-tight text-primary"
									: "mt-6 font-mono text-5xl tracking-tight"
							}
						>
							{plan.price}
						</p>
						<p className="mt-2 font-mono text-xs text-muted-foreground">
							{plan.period}
						</p>
						<ul className="my-8 space-y-4 text-sm">
							{plan.features.map((feature) => (
								<li key={feature} className="flex gap-2">
									<Check size={15} className="mt-1 shrink-0 text-primary" />
									{feature}
								</li>
							))}
						</ul>
						<div className="mt-auto">
							<Button text={plan.cta} containerClassName="min-w-0 w-full" />
							<p className="mt-3 text-center font-mono text-[11px] text-muted-foreground">
								{plan.note}
							</p>
						</div>
					</section>
				))}
			</div>
			<section className="flex flex-col gap-6 border-b border-dotted border-border px-5 py-10 md:flex-row md:items-center md:justify-between md:px-16">
				<div>
					<p className="editorial-label">For teams</p>
					<h2 className="mt-3 text-3xl">Enterprise</h2>
					<p className="mt-2 text-muted-foreground">
						More machine minutes, larger computers, invoicing, and support for
						your team.
					</p>
				</div>
				<Button
					text="Book a call"
					href="https://cal.com/swaraj/15min"
					icon={<CalendarDays size={16} />}
					containerClassName="min-w-0 w-full shrink-0 md:w-auto md:min-w-52"
				/>
			</section>
			<section className="px-5 py-16 md:px-16">
				<h2 className="text-3xl">What you pay for</h2>
				<div className="mt-8 grid gap-4 md:grid-cols-3">
					{costs.map((cost) => (
						<div
							key={cost.title}
							className="rounded-lg border border-dotted border-border p-6"
						>
							<p className="font-mono text-2xl tracking-tight text-primary">
								{cost.title}
							</p>
							<p className="mt-3 text-sm leading-relaxed text-muted-foreground">
								{cost.body}
							</p>
						</div>
					))}
				</div>
				<p className="mt-8 max-w-3xl text-sm leading-relaxed text-muted-foreground">
					One machine minute is one minute of a 1 vCPU / 4 GB computer. A
					Standard 4 vCPU / 16 GB computer uses 4 per awake minute and a Large 8
					vCPU / 32 GB computer uses 8. Cloud prices are in USD, before
					applicable taxes.
				</p>
				<div className="mt-12 overflow-x-auto">
					<table className="w-full text-left text-xs md:text-sm">
						<thead>
							<tr className="border-b border-dotted border-border font-mono text-xs uppercase text-muted-foreground">
								<th className="py-3 pr-2 font-medium md:pr-4">
									<span className="sr-only">Plan</span>
								</th>
								{columns.map((column) => (
									<th
										key={column}
										className={
											column === "Cloud"
												? "py-3 pr-2 font-medium md:pr-4 text-primary"
												: "py-3 pr-2 font-medium md:pr-4"
										}
									>
										{column}
									</th>
								))}
							</tr>
						</thead>
						<tbody>
							{comparison.map(([label, ...values]) => (
								<tr key={label} className="border-b border-dotted border-border">
									<th className="py-3 pr-2 font-normal text-muted-foreground md:pr-4">
										{label}
									</th>
									{values.map((value, index) => (
										<td
											key={columns[index]}
											className="py-3 pr-2 font-mono text-[11px] md:pr-4 md:text-xs"
										>
											{value}
										</td>
									))}
								</tr>
							))}
						</tbody>
					</table>
				</div>
			</section>
		</main>
	);
}
