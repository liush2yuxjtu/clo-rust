pub(super) struct Family {
    pub name: &'static str,
    pub members: &'static [(&'static str, i32, bool)],
}

pub(super) const FAMILIES: &[Family] = &[
    Family {
        name: "ButtonPairEdgeOrigin",
        members: &[
            ("BUTTON_PAIR_EDGE_ORIGIN_TOP", 0, true),
            ("BUTTON_PAIR_EDGE_ORIGIN_BOTTOM", 1, true),
        ],
    },
    Family {
        name: "CloApiAnchorPoint",
        members: &[
            ("CLOAPI_ANCHOR_CENTER", 0, true),
            ("CLOAPI_ANCHOR_UP", 1, true),
            ("CLOAPI_ANCHOR_RIGHT_UP", 2, true),
            ("CLOAPI_ANCHOR_RIGHT", 3, true),
            ("CLOAPI_ANCHOR_RIGHT_DOWN", 4, true),
            ("CLOAPI_ANCHOR_DOWN", 5, true),
            ("CLOAPI_ANCHOR_LEFT_DOWN", 6, true),
            ("CLOAPI_ANCHOR_LEFT", 7, true),
            ("CLOAPI_ANCHOR_LEFT_UP", 8, true),
        ],
    },
    Family {
        name: "CloApiGuiFormFieldType",
        members: &[
            ("GUI_FIELD_TEXT", 0, true),
            ("GUI_FIELD_CHOICE", 1, true),
            ("GUI_FIELD_CHECK", 2, true),
            ("GUI_FIELD_NUMBER", 3, true),
        ],
    },
    Family {
        name: "ExportTechPackType",
        members: &[
            ("CLO_TECH_PACK", 0, true),
            ("CLO_SET_TECH_PACK", 1, true),
            ("CLO_SET_SREST", 2, true),
            ("CLO_API_TECH_PACK", 3, true),
            ("CLO_DUMMY", 4, true),
        ],
    },
    Family {
        name: "GraphicRemoveMode",
        members: &[
            ("GRAPHIC_REMOVE_BROWSER_ONLY", 0, true),
            ("GRAPHIC_REMOVE_PATTERN_ONLY", 1, true),
            ("GRAPHIC_REMOVE_PATTERN_AND_BROWSER", 2, true),
        ],
    },
    Family {
        name: "HI_RES_TOLERANCE_TYPE",
        members: &[
            ("HI_RES_TOLERANCE_PARTICLE_DISTANCE", 0, true),
            ("HI_RES_TOLERANCE_COLLISION_THICKNESS", 1, true),
            ("HI_RES_TOLERANCE_SKIN_OFFSET", 2, true),
        ],
    },
    Family {
        name: "MaterialTargetComponent",
        members: &[
            ("COMPONENT_FRONT", 0, true),
            ("COMPONENT_BACK", 1, true),
            ("COMPONENT_SIDE", 2, true),
            ("COMPONENT_BUTTON", 3, true),
            ("COMPONENT_THREAD", 4, true),
            ("COMPONENT_NONE", -1, true),
        ],
    },
    Family {
        name: "OBJECT_BROWSER_CATEGORY",
        members: &[
            ("OB_FABRIC", 0, true),
            ("OB_BUTTON", 1, true),
            ("OB_TOPSTITCH", 2, true),
            ("OB_TRIM", 3, true),
            ("OB_ZIPPER", 4, true),
            ("OB_GRAPHIC", 5, true),
            ("OB_BUTTONHOLE", 6, true),
            ("OB_PUCKERING", 7, true),
        ],
    },
    Family {
        name: "ObjectBrowserType",
        members: &[
            ("OB_TYPE_FABRIC", 0, true),
            ("OB_TYPE_GRAPHIC", 1, true),
            ("OB_TYPE_TRIM", 2, true),
            ("OB_TYPE_BUTTON", 3, true),
            ("OB_TYPE_BUTTON_HOLE", 4, true),
            ("OB_TYPE_PUCKERING", 5, true),
            ("OB_TYPE_ZIPPER", 6, true),
            ("OB_TYPE_POM", 7, true),
            ("OB_TYPE_GRADING", 8, true),
            ("OB_TYPE_PATTERN", 9, true),
            ("OB_TYPE_TOPSTITCH", 10, true),
            ("OB_TYPE_AVATAR", 11, true),
        ],
    },
    Family {
        name: "SeamlinePairGroupResult",
        members: &[
            ("SEAMLINE_PAIR_SUCCESS", 0, true),
            ("SEAMLINE_PAIR_ERROR_NO_PATTERN_EDITOR", -1, true),
            ("SEAMLINE_PAIR_ERROR_INVALID_PATTERN", -2, true),
            ("SEAMLINE_PAIR_ERROR_INVALID_INNER_SHAPE", -3, true),
            ("SEAMLINE_PAIR_ERROR_INVALID_LINE", -4, true),
            ("SEAMLINE_PAIR_ERROR_RATIO_OUT_OF_RANGE", -5, true),
            ("SEAMLINE_PAIR_ERROR_EMPTY_SEGMENT", -6, true),
            ("SEAMLINE_PAIR_ERROR_CREATE_FAILED", -7, true),
        ],
    },
    Family {
        name: "TextureMapTarget",
        members: &[
            ("TEXTURE_MAP_BASE_COLOR", 0, true),
            ("TEXTURE_MAP_NORMAL", 1, true),
            ("TEXTURE_MAP_OPACITY", 2, true),
            ("TEXTURE_MAP_ROUGHNESS", 3, true),
            ("TEXTURE_MAP_METALNESS", 4, true),
            ("TEXTURE_MAP_DISPLACEMENT", 5, true),
        ],
    },
    Family {
        name: "WELD_TYPE",
        members: &[
            ("FULLY_UNWELDED", 0, true),
            ("DEFAULT_WELDED", 1, true),
            ("FULLY_WELDED", 2, true),
            ("SELECTED_WELDED", 3, true),
        ],
    },
    Family {
        name: "ZipperObjectType",
        members: &[
            ("ZIPPER_OBJECT_SLIDER", 0, true),
            ("ZIPPER_OBJECT_PULLER", 1, true),
            ("ZIPPER_OBJECT_SLIDER_PULLER", 2, true),
            ("ZIPPER_OBJECT_TEETH", 3, true),
            ("ZIPPER_OBJECT_TOP_STOPPER", 4, true),
            ("ZIPPER_OBJECT_BOTTOM_OPEN_STOPPER", 5, true),
            ("ZIPPER_OBJECT_BOTTOM_CLOSED_STOPPER", 6, true),
        ],
    },
    Family {
        name: "ZipperScaleUnit",
        members: &[
            ("ZIPPER_SCALE_MM", 0, true),
            ("ZIPPER_SCALE_CM", 1, true),
            ("ZIPPER_SCALE_INCH", 2, true),
            ("ZIPPER_SCALE_FEET", 3, true),
            ("ZIPPER_SCALE_M", 4, true),
            ("ZIPPER_SCALE_8_FEET", 5, true),
            ("ZIPPER_SCALE_8_6_FEET", 6, true),
            ("ZIPPER_SCALE_AUTO", 7, true),
        ],
    },
];

pub(super) fn family(name: &str) -> Option<&'static Family> {
    let name = name.strip_prefix("ApiTypes.")?;
    FAMILIES.iter().find(|family| family.name == name)
}

pub(super) fn member(name: &str) -> Option<(&'static str, &'static str, i32)> {
    let name = name.strip_prefix("ApiTypes.")?;
    if let Some((family_name, member_name)) = name.split_once('.') {
        let family = FAMILIES.iter().find(|family| family.name == family_name)?;
        return family
            .members
            .iter()
            .find(|(name, _, _)| *name == member_name)
            .map(|(name, value, _)| (family.name, *name, *value));
    }
    FAMILIES.iter().find_map(|family| {
        family
            .members
            .iter()
            .find(|(member, _, global)| *global && *member == name)
            .map(|(name, value, _)| (family.name, *name, *value))
    })
}
