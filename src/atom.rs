use x11rb::atom_manager;

atom_manager! {
    pub AtomCollection: AtomCollectionCookie {
        WM_STATE,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DIALOG,
    }
}
