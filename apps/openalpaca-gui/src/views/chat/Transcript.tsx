/**
 * The transcript column (DESIGN_SPEC §2.2, §5.1).
 *
 * Pure rendering of the ordered items `buildTranscript` produced: message rows
 * carry the density gap themselves, the run-report card and the confirmation
 * banner carry their own 26px, and nothing here fetches anything except the
 * per-artifact file metadata each card needs.
 */

import {
  AssistantMessage,
  ResolutionRow,
  RunReportCard,
  ToolConfirmationBanner,
  UserMessage,
  WrittenArtifactCard,
  formatClock,
  formatElapsed,
  shortRunId,
} from "@/components/chat";
import { languageFromName, toFileKind } from "@/components/ui";
import type { ArtifactKind } from "@/lib/api/artifacts";
import { useUiStore } from "@/stores/ui";

import { TranscriptArtifact } from "./TranscriptArtifact";
import type { TranscriptItem } from "./transcript-model";

export interface TranscriptProps {
  items: readonly TranscriptItem[];
  dense: boolean;
}

export function Transcript({ items, dense }: TranscriptProps) {
  const openArtifact = useUiStore((s) => s.openArtifact);
  return (
    <>
      {items.map((item) => {
        switch (item.kind) {
          case "user":
            return (
              <UserMessage
                key={item.key}
                text={item.text}
                time={formatClock(item.time)}
                steer={item.steer}
                dense={dense}
              />
            );

          case "assistant":
            return (
              <AssistantMessage
                key={item.key}
                text={item.text}
                meta={item.meta}
                streamPhase={item.streamPhase}
                dense={dense}
              >
                {item.attachments.map((attachment) => (
                  <TranscriptArtifact
                    key={attachment.fileId}
                    attachment={attachment}
                  />
                ))}
              </AssistantMessage>
            );

          case "report":
            return (
              <RunReportCard
                key={item.key}
                status={item.report.status}
                time={formatClock(item.report.endedAt)}
                runId={shortRunId(item.report.taskId)}
                duration={formatElapsed(
                  item.report.startedAt,
                  item.report.endedAt,
                )}
                title={item.report.title}
                summary={item.report.summary}
                note={
                  item.report.artifactCount > 0
                    ? `${item.report.artifactCount} file${item.report.artifactCount === 1 ? "" : "s"} produced — in the Library`
                    : null
                }
              />
            );

          case "artifact":
            return (
              <WrittenArtifactCard
                key={item.key}
                name={item.entry.name}
                kind={toFileKind(item.entry.kind as ArtifactKind)}
                language={languageFromName(item.entry.name)}
                version={item.entry.version}
                time={formatClock(item.entry.at)}
                onOpen={() => openArtifact(item.entry.artifactId)}
              />
            );

          case "confirmation":
            return (
              <ToolConfirmationBanner
                key={item.key}
                toolName={item.entry.toolName}
                toolArguments={item.entry.toolArguments}
                agentName={item.entry.agentName}
              />
            );

          case "resolution":
            return (
              <ResolutionRow
                key={item.key}
                resolution={item.entry.resolution}
                note={item.entry.note}
                time={formatClock(item.entry.at)}
              />
            );

          case "error":
            return (
              <div
                key={item.key}
                role="alert"
                className="mb-[26px] rounded-xl border border-red-line bg-red-tint px-[13px] py-[11px]"
              >
                <p className="mt-0 mb-[5px] font-mono text-2xs-plus tracking-eyebrow text-red-ink uppercase">
                  Stream ended
                </p>
                <p className="m-0 text-base-plus text-secondary">
                  {item.message}
                </p>
              </div>
            );
        }
      })}
    </>
  );
}
