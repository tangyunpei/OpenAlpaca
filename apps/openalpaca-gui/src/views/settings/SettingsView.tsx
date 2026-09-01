/**
 * The Settings view (DESIGN_SPEC §2.5, §5.4).
 *
 * A 220px section nav over a 660px-max body; the body is the only scrolling
 * region. The nav counts are the design's own trailing numerals, and every one
 * of them is real — a section whose count has not loaded shows none rather than
 * a zero, because a zero is a claim.
 *
 * The eight section bodies live one file each; this only routes between them.
 */

import { useAgentTemplates } from "@/hooks/useAgents";
import { useConnectors } from "@/hooks/useConnectors";
import { useConversations } from "@/hooks/useConversations";
import { usePlugins } from "@/hooks/usePlugins";
import { useLlmSettings } from "@/hooks/useSettings";
import { useSkillHealth } from "@/hooks/useSkills";
import { useUiStore } from "@/stores/ui";

import { AgentsSection } from "./AgentsSection";
import { ConnectionSection } from "./ConnectionSection";
import { ConnectorsSection } from "./ConnectorsSection";
import { ConversationsSection } from "./ConversationsSection";
import { EventLogSection } from "./EventLogSection";
import { ModelsSection } from "./ModelsSection";
import { PluginsSection } from "./PluginsSection";
import { SkillsSection } from "./SkillsSection";
import { PageHead, SectionNavItem } from "./primitives";
import {
  SETTINGS_SECTIONS,
  sectionMeta,
  toSectionId,
  type SettingsSectionId,
} from "./sections";

function renderSection(id: SettingsSectionId) {
  switch (id) {
    case "connection":
      return <ConnectionSection />;
    case "models":
      return <ModelsSection />;
    case "connectors":
      return <ConnectorsSection />;
    case "skills":
      return <SkillsSection />;
    case "plugins":
      return <PluginsSection />;
    case "agents":
      return <AgentsSection />;
    case "conversations":
      return <ConversationsSection />;
    case "events":
      return <EventLogSection />;
  }
}

/** `undefined` where the list has not loaded — never `0`. */
function useSectionCounts(): Partial<Record<SettingsSectionId, number>> {
  const llm = useLlmSettings();
  const connectors = useConnectors();
  const skills = useSkillHealth();
  const plugins = usePlugins();
  const templates = useAgentTemplates();
  const conversations = useConversations({ limit: 50 });

  return {
    models:
      llm.data === undefined
        ? undefined
        : Object.keys(llm.data.providers).length,
    connectors: connectors.data?.length,
    skills: skills.data?.length,
    plugins: plugins.data?.length,
    agents: templates.data?.length,
    conversations: conversations.data?.conversations.length,
  };
}

export default function SettingsView() {
  const rawSectionId = useUiStore((s) => s.settingsSectionId);
  const setSection = useUiStore((s) => s.setSettingsSection);
  const active = toSectionId(rawSectionId);
  const meta = sectionMeta(active);
  const counts = useSectionCounts();

  return (
    <section aria-label="Settings" className="flex min-w-0 flex-1 bg-main">
      <nav
        aria-label="Settings sections"
        className="flex w-settings-nav shrink-0 flex-col border-r border-line-subtle px-[12px] py-[16px]"
      >
        <h2 className="m-0 mb-[14px] px-[8px] text-lg-plus font-semibold text-ink">
          Settings
        </h2>
        <div className="flex flex-col gap-[2px]">
          {SETTINGS_SECTIONS.map((section) => (
            <SectionNavItem
              key={section.id}
              label={section.label}
              active={section.id === active}
              count={counts[section.id]}
              onSelect={() => setSection(section.id)}
            />
          ))}
        </div>
      </nav>

      <div className="sc min-h-0 flex-1 overflow-y-auto px-[32px] pt-[26px] pb-[34px]">
        <div className="max-w-settings-max">
          <PageHead title={meta.label} blurb={meta.blurb} />
          {renderSection(active)}
        </div>
      </div>
    </section>
  );
}
