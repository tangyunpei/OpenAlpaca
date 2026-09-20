/**
 * Table renderer (DESIGN_SPEC §3.25d).
 *
 * Not a `<table>`: the design lays the grid out with flex weights per column,
 * which a table element cannot reproduce without fixed widths. It is exposed
 * to assistive tech with explicit table roles instead.
 *
 * Two per-column decisions are data-driven, not hand-annotated: identifier and
 * numeric columns render mono, and a column whose every value is yes/no gets
 * the green/red treatment — §3.25d's "plain `yes` in a non-status column stays
 * default ink".
 */

import { useMemo } from "react";

import { cn } from "@/lib/cn";

import {
  columnFlex,
  columnValues,
  isBooleanColumn,
  isMonoColumn,
} from "./parse";
import { PreviewShell } from "./PreviewShell";
import type { PreviewSize, TableModel } from "./types";

export interface TablePreviewProps {
  table: TableModel;
  size: PreviewSize;
  className?: string;
}

interface ColumnStyle {
  flex: number;
  mono: boolean;
  boolean: boolean;
}

export function TablePreview({ table, size, className }: TablePreviewProps) {
  const full = size === "full";
  const columns = useMemo<ColumnStyle[]>(
    () =>
      table.columns.map((_column, index) => {
        const values = columnValues(table, index);
        return {
          flex: columnFlex(index, table.columns.length, size),
          mono: isMonoColumn(values),
          boolean: isBooleanColumn(values),
        };
      }),
    [table, size],
  );

  return (
    <PreviewShell size={size} className={className}>
      <div role="table" className="min-w-0">
        <div
          role="row"
          className={cn(
            "flex border-b border-line-subtle bg-sunken font-mono text-muted-fg uppercase",
            full ? "text-xs tracking-label" : "text-2xs tracking-[.05em]",
          )}
        >
          {table.columns.map((column, index) => (
            <span
              key={column === "" ? `col-${index}` : column}
              role="columnheader"
              className={cn(
                "min-w-0 truncate",
                full ? "px-[14px] py-[9px]" : "px-[11px] py-[7px]",
              )}
              style={{ flex: columns[index]?.flex ?? 1 }}
            >
              {column}
            </span>
          ))}
        </div>

        {table.rows.map((row, rowIndex) => (
          <div
            // Row order is the identity; a CSV row has no id.
            key={rowIndex}
            role="row"
            className={cn(
              "flex",
              full ? "text-base-plus" : "text-sm-plus",
              rowIndex < table.rows.length - 1 && "border-b border-line-hair-2",
            )}
          >
            {row.map((cell, cellIndex) => {
              const style = columns[cellIndex];
              const bool =
                style?.boolean === true ? cell.trim().toLowerCase() : "";
              return (
                <span
                  key={cellIndex}
                  role="cell"
                  className={cn(
                    "min-w-0 truncate",
                    full ? "px-[14px] py-[10px]" : "px-[11px] py-[8px]",
                    style?.mono === true &&
                      (full
                        ? "font-mono text-sm-plus"
                        : "font-mono text-xs-plus"),
                    bool === "yes" && "text-green",
                    bool === "no" && "text-red",
                  )}
                  style={{ flex: style?.flex ?? 1 }}
                >
                  {cell}
                </span>
              );
            })}
          </div>
        ))}
      </div>
    </PreviewShell>
  );
}
