use x11rb::atom_manager;

atom_manager! {
    pub AtomCollection: AtomCollectionCookie {
        WM_STATE,
    }
}
