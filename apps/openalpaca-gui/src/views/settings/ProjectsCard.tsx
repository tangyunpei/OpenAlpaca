/**
 * Settings → Connection → Projects (storage design §6, §9; ST-7, ST-20).
 *
 * Every project the home store records — one folder under `projects/` each,
 * whose `project.json` holds its id and its root (ST-13) — as
 * `GET /v1/workspaces` lists it, by folder name. A project whose root is no
 * longer a directory is marked `missing` and keeps everything recorded for
 * it (ST-7): nothing detects a move, and nothing is dropped on its own.
 * `Re-point…` is how the owner says where a moved folder went.
 *
 * The list is re-read whenever it mounts and after each write made here: no
 * event announces a project that a chat turn or an upload registered, and
 * polling a list that changes this rarely would only cost the daemon.
 *
 * A window follows its own project: after a re-point the old root is no
 * longer registered, so a window still sending it would file its next turn
 * under no project. It takes the new root.
 */

import { useEffect, useRef, useState } from "react";

import {
  RepointProjectDialog,
  repointTarget,
  rowsLine,
} from "@/components/overlays/RepointProjectDialog";
import { Button, Tag } from "@/components/ui";
import {
  useProjects,
  useRepointProject,
  useWorkspace,
} from "@/hooks/useWorkspaces";
import type { Project } from "@/lib/api/workspaces";
import { useProjectStore } from "@/stores/project";
import { useUiStore } from "@/stores/ui";

import { pickDirectory } from "./AddExtensionForm";
import { ListCard, ListRow, ListState } from "./primitives";

/** What the card says when the home store records no project yet. */
export const PROJECTS_EMPTY_COPY =
  "No projects yet. A folder becomes one when you Use it above, or the first time you chat from inside a git repository.";

/** What a missing project's row says under its root (ST-7). */
export const MISSING_PROJECT_COPY =
  "Its folder is gone. Everything recorded for it is kept until you re-point it or delete its data.";

export interface ProjectsCardProps {
  /** `GET /v1/status.project_root` — the project this window's turns go to. */
  currentRoot: string | null;
}

type OpenAction = { kind: "repoint"; project: Project } | null;

export function ProjectsCard({ currentRoot }: ProjectsCardProps) {
  const projects = useProjects();
  const rows = projects.data ?? [];
  const [open, setOpen] = useState<OpenAction>(null);

  return (
    <section aria-label="Projects" className="flex flex-col gap-[8px]">
      <h3 className="m-0 text-md-plus font-semibold text-ink">Projects</h3>
      <ListCard>
        <ListState
          pending={projects.isPending}
          error={projects.error}
          empty={rows.length === 0}
          emptyCopy={PROJECTS_EMPTY_COPY}
        >
          {rows.map((project) => (
            <ProjectRow
              key={project.id}
              project={project}
              current={project.root === currentRoot}
              onRepoint={() => setOpen({ kind: "repoint", project })}
            />
          ))}
        </ListState>
      </ListCard>

      {open?.kind === "repoint" && (
        <RepointFlow
          project={open.project}
          current={open.project.root === currentRoot}
          onClose={() => setOpen(null)}
        />
      )}
    </section>
  );
}

function ProjectRow({
  project,
  current,
  onRepoint,
}: {
  project: Project;
  current: boolean;
  onRepoint: () => void;
}) {
  return (
    <ListRow
      name={<span className="font-mono">{project.root}</span>}
      wrapName
      tags={
        <>
          {project.missing && <Tag value="missing" tone="warn" />}
          {current && <Tag value="this window" tone="live" />}
        </>
      }
      description={project.missing ? MISSING_PROJECT_COPY : undefined}
      meta={project.name}
      wrapMeta
      control={
        <span className="flex shrink-0 gap-[6px]">
          <Button variant="ghostSm" onClick={onRepoint}>
            Re-point…
          </Button>
        </span>
      }
    />
  );
}

interface FlowProps {
  project: Project;
  /** This window's turns go to `project` (`GET /v1/status.project_root`). */
  current: boolean;
  onClose: () => void;
}

/**
 * The re-point, mounted only while its dialog is open. The counts are read
 * fresh on open, as `StopDaemonFlow` re-reads `busy`: the count confirmed is
 * the one on screen, not a cached one.
 */
function RepointFlow({ project, current, onClose }: FlowProps) {
  const counts = useWorkspace(project.root);
  const repoint = useRepointProject();
  const windowPath = useProjectStore((s) => s.path);
  const setPath = useProjectStore((s) => s.setPath);
  const showToast = useUiStore((s) => s.showToast);
  const [value, setValue] = useState("");
  const [refusal, setRefusal] = useState<string | null>(null);
  const [reading, setReading] = useState(true);

  const refetch = useRef(counts.refetch);
  useEffect(() => {
    let live = true;
    void Promise.resolve(refetch.current()).finally(() => {
      if (live) setReading(false);
    });
    return () => {
      live = false;
    };
  }, []);

  function confirm() {
    const target = repointTarget(value, project);
    if (target === null) return;
    setRefusal(null);
    repoint.mutate(
      { from: project.root, to: target },
      {
        onSuccess: (result) => {
          if (current || windowPath === project.root) {
            setPath(result.project.root);
          }
          showToast(
            `Re-pointed to ${result.project.root}: ${rowsLine(result.moved)}.`,
          );
          onClose();
        },
        onError: (error) => setRefusal(error.message),
      },
    );
  }

  return (
    <RepointProjectDialog
      project={project}
      rows={counts.data?.rows}
      activeTasks={counts.data?.active_tasks ?? 0}
      reading={reading}
      readError={counts.error?.message ?? null}
      value={value}
      onChange={(next) => {
        setValue(next);
        setRefusal(null);
      }}
      onBrowse={() => {
        void pickDirectory().then((picked) => {
          if (picked === null) return;
          setValue(picked);
          setRefusal(null);
        });
      }}
      refusal={refusal}
      repointing={repoint.isPending}
      onCancel={onClose}
      onConfirm={confirm}
    />
  );
}
