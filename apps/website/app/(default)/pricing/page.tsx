import { CalendarDays, Check } from "lucide-react";
import { Button } from "@/components/button";
import { PageMasthead } from "@/components/page-masthead";
import { getSEO } from "@/lib/seo";

export const metadata = getSEO({
	title: "Pricing: local and cloud coding agents",
	description:
		"Use Zuse locally for free during beta. Zuse Cloud is $20/month with 33,600 machine minutes included, or $50/month for Cloud Max. Try the first week for $1. Bring your own agent subscriptions.",
	path: "/pricing",
});

const plans = [
	{
		name: "Local",
		price: "$0",
		period: "during beta",
		summary: "Your machine. Your agents. Your workflow.",
		features: [
			"Open-source desktop app",
			"Parallel chats and Git worktrees",
			"Bring your own agent keys or subscriptions",
			"Local files, terminal, and diff review",
		],
		download: true,
	},
	{
		name: "Cloud",
		price: "$20",
		period: "/ month · USD · first week $1",
		summary: "Isolated cloud computers for work beyond your laptop.",
		features: [
			"33,600 machine minutes a month, about 140 awake hours on the default 4 vCPU / 16 GB machine",
			"Asleep machines use no minutes",
			"Pay as you go past the allowance at the same rate, no markup",
			"Configurable monthly cap; $25 default",
			"Trial: first week for $1 with 8,400 machine minutes and no overage",
		],
		download: false,
	},
	{
		name: "Cloud Max",
		price: "$50",
		period: "/ month · USD",
		summary: "More time and larger machines for heavier cloud work.",
		features: [
			"84,000 machine minutes a month, about 350 awake hours on the default machine",
			"Larger machines, up to 8 vCPU / 32 GB",
			"Pay as you go past the allowance at the same rate, no markup",
			"Configurable monthly cap; $25 default",
		],
		download: false,
	},
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
				description="Use the agents you already pay for. Choose where the work runs."
				artwork="handoff"
			/>
			<div className="grid md:grid-cols-3">
				{plans.map((plan) => (
					<section key={plan.name} className="pricing-card flex flex-col">
						<p className="editorial-label">
							{plan.download ? "On your machine" : "Cloud · Beta"}
						</p>
						<h2 className="mt-4 text-3xl">{plan.name}</h2>
						<p className="mt-6 font-mono text-4xl tracking-tight">
							{plan.price}
						</p>
						<p className="mt-2 font-mono text-xs text-muted-foreground">
							{plan.period}
						</p>
						<p className="mt-6 min-h-14 text-muted-foreground">
							{plan.summary}
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
							{plan.download ? (
								<Button containerClassName="min-w-0 w-full" />
							) : (
								<Button
									text="Book a call"
									href="https://cal.com/swaraj/15min"
									icon={<CalendarDays size={16} />}
									containerClassName="min-w-0 w-full"
								/>
							)}
						</div>
					</section>
				))}
			</div>
			<section className="grid gap-10 px-5 py-16 md:grid-cols-2 md:px-16">
				<div>
					<h2 className="text-3xl">What is a machine minute?</h2>
					<p className="mt-4 text-muted-foreground leading-relaxed">
						One machine minute is one minute of a 1 vCPU / 4 GB machine. The
						default 4 vCPU / 16 GB machine uses 4 per awake minute and an 8 vCPU
						/ 32 GB machine uses 8. Asleep machines use none. Extra minutes cost
						the same as included ones: $20 per 33,600. Machine minutes cover
						cloud computers, not model tokens; agent subscriptions and API usage
						are billed separately by your provider. Cloud prices are in USD,
						before applicable taxes.
					</p>
				</div>
				<div>
					<h2 className="text-3xl">Can I stay local?</h2>
					<p className="mt-4 text-muted-foreground leading-relaxed">
						Yes. Cloud is optional. Download the desktop app and connect your
						existing coding agents. Zuse Cloud is in beta: the first week of
						Cloud costs $1 and includes 8,400 machine minutes, then it renews as
						Cloud at $20/month. Book a call to discuss availability and setup.
					</p>
				</div>
			</section>
		</main>
	);
}
