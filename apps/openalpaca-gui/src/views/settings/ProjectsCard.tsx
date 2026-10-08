/**
 * Settings → Connection → Projects (storage design §6, §9; ST-7).
 *
 * Every project the home store records — one folder under `projects/` each,
 * whose `project.json` holds its id and its root (ST-13) — as
 * `GET /v1/workspaces` lists it, by folder name. A project whose root is no
 * longer a directory is marked `missing` and keeps everything recorded for
 * it (ST-7): nothing detects a move, and nothing is dropped on its own.
 *
 * The list is re-read whenever it mounts and after each write made here: no
 * event announces a project that a chat turn or an upload registered, and
 * polling a list that changes this rarely would only cost the daemon.
 */

import { Tag } from "@/components/ui";
import { useProjects } from "@/hooks/useWorkspaces";
import type { Project } from "@/lib/api/workspaces";

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

export function ProjectsCard({ currentRoot }: ProjectsCardProps) {
  const projects = useProjects();
  const rows = projects.data ?? [];

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
            />
          ))}
        </ListState>
      </ListCard>
    </section>
  );
}

function ProjectRow({
  project,
  current,
}: {
  project: Project;
  current: boolean;
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
    />
  );
}
