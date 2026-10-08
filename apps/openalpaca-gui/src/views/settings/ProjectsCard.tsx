/**
 * Settings → Connection → Projects (storage design §6, §9; ST-7, ST-20).
 *
 * Every project the home store records — one folder under `projects/` each,
 * whose `project.json` holds its id and its root (ST-13) — as
 * `GET /v1/workspaces` lists it, by folder name. A project whose root is no
 * longer a directory is marked `missing` and keeps everything recorded for
 * it (ST-7): nothing detects a move, and nothing is dropped on its own.
 * `Re-point…` is how the owner says where a moved folder went, and
 * `Delete data…` removes everything OpenAlpaca holds for a project (ST-19).
 *
 * The list is re-read whenever it mounts and after each write made here: no
 * event announces a project that a chat turn or an upload registered, and
 * polling a list that changes this rarely would only cost the daemon.
 *
 * A window follows its own project through both actions. After a re-point
 * the old root is no longer registered, so a window still sending it would
 * file its next turn under no project: it takes the new root. After a
 * delete, a window still sending the root would register it again, empty,
 * on its next turn: it lets go of the project, and the toast says so —
 * unless the daemon kept the project's folder (`project_folder: false`, as
 * when not every memory partition could be dropped), in which case the
 * project is still registered and listed, the window keeps it, and the
 * toast says to delete its data again.
 */

import { useEffect, useRef, useState } from "react";

import {
  RepointProjectDialog,
  repointTarget,
  rowsLine,
} from "@/components/overlays/RepointProjectDialog";
import { DeleteProjectDataDialog } from "@/components/overlays/DeleteProjectDataDialog";
import { Button, Tag } from "@/components/ui";
import {
  useDeleteProjectData,
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

type OpenAction = { kind: "repoint" | "delete"; project: Project } | null;

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
              onDelete={() => setOpen({ kind: "delete", project })}
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
      {open?.kind === "delete" && (
        <DeleteFlow
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
  onDelete,
}: {
  project: Project;
  current: boolean;
  onRepoint: () => void;
  onDelete: () => void;
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
          <Button variant="ghostSm" onClick={onDelete}>
            Delete data…
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
 * Whether a flow is still on screen. A write's answer can land after Cancel,
 * Escape, the veil or a view switch (⌘1–4, ⌘,) unmounted the flow: what the
 * window does about the write still happens, but closing the dialog is no
 * longer this flow's to do — the owner may have opened another since.
 */
function useOnScreen() {
  const onScreen = useRef(false);
  useEffect(() => {
    onScreen.current = true;
    return () => {
      onScreen.current = false;
    };
  }, []);
  return onScreen;
}

/**
 * Whether the window follows `project` once a write about it has landed,
 * read then rather than at the press, since the dialog may be gone by then:
 * the window's stored path is the project's root, or it is still the path it
 * held at the press and that path resolved to the project (`current`).
 */
function windowFollows(
  project: Project,
  current: boolean,
  pathAtPress: string | null,
): boolean {
  const now = useProjectStore.getState().path;
  return now === project.root || (current && now === pathAtPress);
}

/**
 * The re-point, mounted only while its dialog is open. The counts are read
 * fresh on open, as `StopDaemonFlow` re-reads `busy`: the count confirmed is
 * the one on screen, not a cached one.
 *
 * The answer is taken from `mutateAsync`'s promise, not from `mutate`'s
 * callbacks: query-core runs those only while the observer has a listener,
 * so they die with this flow, while the daemon finishes the write whether or
 * not the dialog is still open. The window follows and the toast says so
 * either way.
 */
function RepointFlow({ project, current, onClose }: FlowProps) {
  const counts = useWorkspace(project.root);
  const repoint = useRepointProject();
  const windowPath = useProjectStore((s) => s.path);
  const setPath = useProjectStore((s) => s.setPath);
  const showToast = useUiStore((s) => s.showToast);
  const onScreen = useOnScreen();
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
    // R1: never on counts the daemon read as another project's (the dialog
    // keeps the press off; this keeps the handler off too).
    const read = counts.data ?? null;
    if (read !== null && read.id !== project.id) return;
    setRefusal(null);
    const pathAtPress = windowPath;
    void repoint.mutateAsync({ from: project.root, to: target }).then(
      (result) => {
        if (windowFollows(project, current, pathAtPress)) {
          setPath(result.project.root);
        }
        showToast(
          `Re-pointed to ${result.project.root}: ${rowsLine(result.moved)}.`,
        );
        if (onScreen.current) onClose();
      },
      (error: Error) => setRefusal(error.message),
    );
  }

  return (
    <RepointProjectDialog
      project={project}
      rows={counts.data?.rows}
      resolvedAs={counts.data ?? undefined}
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

/**
 * The delete, mounted only while its dialog is open: one dry run on open
 * for the plan, and the delete itself only on the press. The two calls are
 * two mutations, so the plan stays on screen while the delete is in flight.
 *
 * A delete can answer `200` with `removed.project_folder: false`: the daemon
 * kept `projects/<name>/` — not every memory partition could be dropped, or
 * the folder itself could not be removed — so the project stays registered
 * and listed, and deleting its data again finishes it. That answer is never drawn as the project being gone — the
 * window keeps the project it still is, and the toast says to run it again.
 *
 * As in `RepointFlow`, the delete's answer is taken from `mutateAsync`'s
 * promise, so a dialog closed while `Deleting…` (up to 3 s of quiesce, then
 * the delete) still leaves the window without the project and says so.
 */
function DeleteFlow({ project, current, onClose }: FlowProps) {
  const plan = useDeleteProjectData();
  const apply = useDeleteProjectData();
  const windowPath = useProjectStore((s) => s.path);
  const setPath = useProjectStore((s) => s.setPath);
  const showToast = useUiStore((s) => s.showToast);
  const onScreen = useOnScreen();
  const [refusal, setRefusal] = useState<string | null>(null);

  // One dry run per open: StrictMode runs effects twice in development.
  const asked = useRef(false);
  const askPlan = useRef(plan.mutate);
  useEffect(() => {
    if (asked.current) return;
    asked.current = true;
    askPlan.current({ path: project.root, dryRun: true });
  }, [project.root]);

  function confirm() {
    // R1: never on a plan the daemon read as another project's (the dialog
    // keeps the press off; this keeps the handler off too).
    if (plan.data?.project.id !== project.id) return;
    setRefusal(null);
    const pathAtPress = windowPath;
    void apply.mutateAsync({ path: project.root, dryRun: false }).then(
      (result) => {
        if (result.removed?.project_folder === false) {
          showToast(
            `Deleted the data of ${project.root}, but not all of it could be removed: its folder under projects/ was kept, so the project is still listed. Delete its data again to finish.`,
          );
        } else {
          const follows = windowFollows(project, current, pathAtPress);
          if (follows) setPath(null);
          showToast(
            follows
              ? `Deleted the data of ${project.root}. This window has no project now.`
              : `Deleted the data of ${project.root}.`,
          );
        }
        if (onScreen.current) onClose();
      },
      (error: Error) => setRefusal(error.message),
    );
  }

  return (
    <DeleteProjectDataDialog
      project={project}
      plan={plan.data}
      planError={plan.error?.message ?? null}
      refusal={refusal}
      deleting={apply.isPending}
      onCancel={onClose}
      onConfirm={confirm}
    />
  );
}
