use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct IconProps {
    #[props(default = 20)]
    pub size: u32,
    #[props(default = String::new())]
    pub class: String,
}

#[component]
pub fn IconHouse(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M219.31,108.68l-80-80a16,16,0,0,0-22.62,0l-80,80A15.87,15.87,0,0,0,32,120v96a8,8,0,0,0,8,8H216a8,8,0,0,0,8-8V120A15.87,15.87,0,0,0,219.31,108.68ZM208,208H48V120l80-80,80,80Z"
            }
        }
    }
}

#[component]
pub fn IconClockCounterClockwise(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M136,80v43.47l36.12,21.67a8,8,0,0,1-8.24,13.72l-40-24A8,8,0,0,1,120,128V80a8,8,0,0,1,16,0Zm-8-48A95.44,95.44,0,0,0,60.08,60.15C52.81,67.51,46.35,74.59,40,82V64a8,8,0,0,0-16,0v40a8,8,0,0,0,8,8H72a8,8,0,0,0,0-16H49c7.15-8.42,14.27-16.35,22.39-24.57a80,80,0,1,1,1.66,114.75a8,8,0,1,0-11,11.64A96,96,0,1,0,128,32Z"
            }
        }
    }
}

#[component]
pub fn IconGear(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M128,80a48,48,0,1,0,48,48A48.05,48.05,0,0,0,128,80Zm0,80a32,32,0,1,1,32-32A32,32,0,0,1,128,160Zm88-29.84q.06-2.16,0-4.32l14.92-18.64a8,8,0,0,0,1.48-7.06,107.21,107.21,0,0,0-10.88-26.25,8,8,0,0,0-6-3.93l-23.72-2.64q-1.48-1.56-3-3L186,40.54a8,8,0,0,0-3.94-6,107.71,107.71,0,0,0-26.25-10.87,8,8,0,0,0-7.06,1.49L130.16,40Q128,40,125.84,40L107.2,25.11a8,8,0,0,0-7.06-1.48A107.6,107.6,0,0,0,73.89,34.51a8,8,0,0,0-3.93,6L67.32,64.27q-1.56,1.49-3,3L40.54,70a8,8,0,0,0-6,3.94,107.71,107.71,0,0,0-10.87,26.25,8,8,0,0,0,1.49,7.06L40,125.84Q40,128,40,130.16L25.11,148.8a8,8,0,0,0-1.48,7.06,107.21,107.21,0,0,0,10.88,26.25,8,8,0,0,0,6,3.93l23.72,2.64q1.49,1.56,3,3L70,215.46a8,8,0,0,0,3.94,6,107.71,107.71,0,0,0,26.25,10.87,8,8,0,0,0,7.06-1.49L125.84,216q2.16.06,4.32,0l18.64,14.92a8,8,0,0,0,7.06,1.48,107.21,107.21,0,0,0,26.25-10.88,8,8,0,0,0,3.93-6l2.64-23.72q1.56-1.48,3-3L215.46,186a8,8,0,0,0,6-3.94,107.71,107.71,0,0,0,10.87-26.25,8,8,0,0,0-1.49-7.06Zm-16.1-6.5a73.93,73.93,0,0,1,0,8.68,8,8,0,0,0,1.74,5.48l14.19,17.73a91.57,91.57,0,0,1-6.23,15L187,173.11a8,8,0,0,0-5.1,2.64,74.11,74.11,0,0,1-6.14,6.14,8,8,0,0,0-2.64,5.1l-2.51,22.58a91.32,91.32,0,0,1-15,6.23l-17.74-14.19a8,8,0,0,0-5-1.75h-.48a73.93,73.93,0,0,1-8.68,0,8,8,0,0,0-5.48,1.74L100.45,215.8a91.57,91.57,0,0,1-15-6.23L82.89,187a8,8,0,0,0-2.64-5.1,74.11,74.11,0,0,1-6.14-6.14,8,8,0,0,0-5.1-2.64L46.43,170.6a91.32,91.32,0,0,1-6.23-15l14.19-17.74a8,8,0,0,0,1.74-5.48,73.93,73.93,0,0,1,0-8.68,8,8,0,0,0-1.74-5.48L40.2,100.45a91.57,91.57,0,0,1,6.23-15L69,82.89a8,8,0,0,0,5.1-2.64,74.11,74.11,0,0,1,6.14-6.14A8,8,0,0,0,82.89,69L85.4,46.43a91.32,91.32,0,0,1,15-6.23l17.74,14.19a8,8,0,0,0,5.48,1.74,73.93,73.93,0,0,1,8.68,0,8,8,0,0,0,5.48-1.74L155.55,40.2a91.57,91.57,0,0,1,15,6.23L173.11,69a8,8,0,0,0,2.64,5.1,74.11,74.11,0,0,1,6.14,6.14,8,8,0,0,0,5.1,2.64l22.58,2.51a91.32,91.32,0,0,1,6.23,15l-14.19,17.74A8,8,0,0,0,199.87,123.66Z"
            }
        }
    }
}

#[component]
pub fn IconPencil(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M227.31,73.37,182.63,28.68a16,16,0,0,0-22.63,0L36.69,152A15.86,15.86,0,0,0,32,163.31V208a16,16,0,0,0,16,16H92.69A15.86,15.86,0,0,0,104,219.31L227.31,96a16,16,0,0,0,0-22.63ZM92.69,208H48V163.31l88-88L180.69,120ZM192,108.68,147.31,64l24-24L216,84.68Z"
            }
        }
    }
}

#[component]
pub fn IconTrash(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M216,48H176V40a24,24,0,0,0-24-24H104A24,24,0,0,0,80,40v8H40a8,8,0,0,0,0,16h8V208a16,16,0,0,0,16,16H192a16,16,0,0,0,16-16V64h8a8,8,0,0,0,0-16ZM96,40a8,8,0,0,1,8-8h48a8,8,0,0,1,8,8v8H96Zm96,168H64V64H192ZM112,104v64a8,8,0,0,1-16,0V104a8,8,0,0,1,16,0Zm48,0v64a8,8,0,0,1-16,0V104a8,8,0,0,1,16,0Z"
            }
        }
    }
}

#[component]
pub fn IconBook(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M208,24H72A32,32,0,0,0,40,56V224a8,8,0,0,0,8,8H192a8,8,0,0,0,0-16H56a16,16,0,0,1,16-16H208a8,8,0,0,0,8-8V32A8,8,0,0,0,208,24Zm-8,160H72a31.82,31.82,0,0,0-16,4.29V56A16,16,0,0,1,72,40H200Z"
            }
        }
    }
}

/// Phosphor "note" — a page with a folded corner. Sidebar entry for the notes
/// board, matching the 256x256 currentColor convention of every icon here.
#[component]
pub fn IconNote(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M208,32H48A16,16,0,0,0,32,48V208a16,16,0,0,0,16,16H156.69A15.86,15.86,0,0,0,168,219.31L219.31,168A15.86,15.86,0,0,0,224,156.69V48A16,16,0,0,0,208,32ZM48,48H208v96H160a16,16,0,0,0-16,16v48H48ZM160,196.69V160h36.69Z"
            }
        }
    }
}

#[component]
pub fn IconMinus(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M224,128a8,8,0,0,1-8,8H40a8,8,0,0,1,0-16H216A8,8,0,0,1,224,128Z"
            }
        }
    }
}

#[component]
pub fn IconX(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M205.66,194.34a8,8,0,0,1-11.32,11.32L128,139.31,61.66,205.66a8,8,0,0,1-11.32-11.32L116.69,128,50.34,61.66A8,8,0,0,1,61.66,50.34L128,116.69l66.34-66.35a8,8,0,0,1,11.32,11.32L139.31,128Z"
            }
        }
    }
}

#[component]
pub fn IconCopy(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M216,32H88a8,8,0,0,0-8,8V80H40a8,8,0,0,0-8,8V216a8,8,0,0,0,8,8H168a8,8,0,0,0,8-8V176h40a8,8,0,0,0,8-8V40A8,8,0,0,0,216,32ZM160,208H48V96H160Zm48-48H176V88a8,8,0,0,0-8-8H96V48H208Z"
            }
        }
    }
}

#[component]
pub fn IconCheck(props: IconProps) -> Element {
    let size = props.size.to_string();
    rsx! {
        svg {
            class: "{props.class}",
            width: "{size}",
            height: "{size}",
            view_box: "0 0 256 256",
            fill: "currentColor",
            path {
                d: "M229.66,77.66l-128,128a8,8,0,0,1-11.32,0l-56-56a8,8,0,0,1,11.32-11.32L96,188.69,218.34,66.34a8,8,0,0,1,11.32,11.32Z"
            }
        }
    }
}
