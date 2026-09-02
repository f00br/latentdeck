import { describe, expect, it } from "vitest";

import {
  FaceplateContractError,
  parseFaceplateDefinition,
  validateFaceplateAgainstDeck,
  type FaceplateDeckContract,
} from "./faceplate-model";

const D2_DECK: FaceplateDeckContract = {
  slots: 2,
  role_ids: ["carrier", "donor"],
  controls: [
    {
      control_id: "mix",
      value_type: "number",
      minimum: 0,
      maximum: 1,
      step: 0.01,
    },
    {
      control_id: "routing",
      value_type: "enum",
      options: ["carrier", "donor"],
    },
    { control_id: "interaction", value_type: "boolean" },
  ],
  capabilities: ["snapshot_capture", "live_capture"],
};

function d2Faceplate(): object {
  return {
    schema_version: 1,
    title: "D2",
    sections: [
      {
        section_id: "sources",
        title: "Sources",
        widgets: [
          { id: "source_a", kind: "source_picker", label: "A", slot_index: 0 },
          { id: "source_b", kind: "source_picker", label: "B", slot_index: 1 },
        ],
      },
      {
        section_id: "controls",
        title: "Controls",
        widgets: [
          {
            id: "mix",
            kind: "slider",
            label: "Mix",
            control_id: "mix",
            minimum: 0,
            maximum: 1,
            step: 0.01,
          },
          {
            id: "routing",
            kind: "select",
            label: "Routing",
            control_id: "routing",
            options: [
              { value: "carrier", label: "Carrier" },
              { value: "donor", label: "Donor" },
            ],
          },
          {
            id: "interaction",
            kind: "toggle",
            label: "Interaction",
            control_id: "interaction",
          },
        ],
      },
      {
        section_id: "runtime",
        title: "Runtime",
        widgets: [
          {
            id: "roles",
            kind: "role_editor",
            label: "Roles",
            role_ids: ["carrier", "donor"],
          },
          {
            id: "transport",
            kind: "transport",
            label: "Transport",
            slot_indices: [0, 1],
          },
          { id: "seed", kind: "seed", label: "Seed" },
          {
            id: "capture",
            kind: "capture",
            label: "Capture",
            modes: ["snapshot", "live_capture"],
          },
          { id: "monitor", kind: "monitor", label: "Monitor" },
        ],
      },
    ],
  };
}

describe("closed declarative Deck faceplates", () => {
  it("parses and cross-checks an exact host-rendered D2 surface", () => {
    const faceplate = parseFaceplateDefinition(d2Faceplate());

    expect(() =>
      validateFaceplateAgainstDeck(faceplate, D2_DECK),
    ).not.toThrow();
    expect(
      faceplate.sections.flatMap((section) => section.widgets),
    ).toHaveLength(10);
  });

  it("supports the closed barycentric3 widget for Q4 role binding", () => {
    const source = d2Faceplate() as {
      sections: Array<{
        section_id: string;
        widgets: Array<Record<string, unknown>>;
      }>;
    };
    source.sections[0].widgets = [0, 1, 2, 3].map((slot) => ({
      id: `source_${slot}`,
      kind: "source_picker",
      label: `Source ${slot}`,
      slot_index: slot,
    }));
    source.sections[1].widgets = [
      {
        id: "influence",
        kind: "barycentric3",
        label: "Influence",
        x_control_id: "triangle_x",
        y_control_id: "triangle_y",
        vertex_role_ids: ["donor_b", "donor_c", "donor_d"],
      },
    ];
    source.sections[2].widgets[0].role_ids = [
      "carrier",
      "donor_b",
      "donor_c",
      "donor_d",
    ];
    source.sections[2].widgets[1].slot_indices = [0, 1, 2, 3];
    const q4Deck: FaceplateDeckContract = {
      slots: 4,
      role_ids: ["carrier", "donor_b", "donor_c", "donor_d"],
      controls: [
        {
          control_id: "triangle_x",
          value_type: "number",
          minimum: 0,
          maximum: 1,
          step: 0.001,
        },
        {
          control_id: "triangle_y",
          value_type: "number",
          minimum: 0,
          maximum: 1,
          step: 0.001,
        },
      ],
      capabilities: ["snapshot_capture", "live_capture"],
    };

    const faceplate = parseFaceplateDefinition(source);
    expect(() => validateFaceplateAgainstDeck(faceplate, q4Deck)).not.toThrow();

    const nonNormalizedDeck: FaceplateDeckContract = {
      ...q4Deck,
      controls: q4Deck.controls.map((control) =>
        control.control_id === "triangle_x" && control.value_type === "number"
          ? { ...control, minimum: -1 }
          : control,
      ),
    };
    expect(() =>
      validateFaceplateAgainstDeck(faceplate, nonNormalizedDeck),
    ).toThrowError(
      expect.objectContaining<Partial<FaceplateContractError>>({
        code: "faceplate.control_mismatch",
      }),
    );
  });

  it("rejects executable or open-ended widget fields through closed schemas", () => {
    const source = d2Faceplate() as {
      sections: Array<{ widgets: Array<Record<string, unknown>> }>;
    };
    source.sections[1].widgets[0].html = "<script>invoke('shell')</script>";

    expect(() => parseFaceplateDefinition(source)).toThrowError(
      expect.objectContaining<Partial<FaceplateContractError>>({
        code: "faceplate.closed_schema",
      }),
    );
  });

  it("rejects hidden faceplate/operator coercion or duplicate bindings", () => {
    const wrongBounds = parseFaceplateDefinition(d2Faceplate());
    const changedDeck: FaceplateDeckContract = {
      ...D2_DECK,
      controls: D2_DECK.controls.map((control) =>
        control.control_id === "mix" && control.value_type === "number"
          ? { ...control, maximum: 2 }
          : control,
      ),
    };
    expect(() =>
      validateFaceplateAgainstDeck(wrongBounds, changedDeck),
    ).toThrowError(
      expect.objectContaining<Partial<FaceplateContractError>>({
        code: "faceplate.control_mismatch",
      }),
    );

    const duplicate = d2Faceplate() as {
      sections: Array<{ widgets: Array<Record<string, unknown>> }>;
    };
    duplicate.sections[1].widgets[2].control_id = "mix";
    const parsed = parseFaceplateDefinition(duplicate);
    expect(() => validateFaceplateAgainstDeck(parsed, D2_DECK)).toThrowError(
      expect.objectContaining<Partial<FaceplateContractError>>({
        code: "faceplate.control_mismatch",
      }),
    );

    const duplicateSource = d2Faceplate() as {
      sections: Array<{ widgets: Array<Record<string, unknown>> }>;
    };
    duplicateSource.sections[0].widgets[1].slot_index = 0;
    expect(() =>
      validateFaceplateAgainstDeck(
        parseFaceplateDefinition(duplicateSource),
        D2_DECK,
      ),
    ).toThrowError(
      expect.objectContaining<Partial<FaceplateContractError>>({
        code: "faceplate.duplicate",
      }),
    );

    const duplicateRole = d2Faceplate() as {
      sections: Array<{ widgets: Array<Record<string, unknown>> }>;
    };
    duplicateRole.sections[2].widgets[0].role_ids = [
      "carrier",
      "donor",
      "donor",
    ];
    expect(() => parseFaceplateDefinition(duplicateRole)).toThrowError(
      expect.objectContaining<Partial<FaceplateContractError>>({
        code: "faceplate.invalid_widget",
      }),
    );
  });
});
