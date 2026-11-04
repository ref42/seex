- [x] remove history tab since it is useless for users and it seems no one cares the history.
- [x] add a switch button for monitoring cuz there may be some infos that user did not want seex to capture, and hide `Quick ID` and `Full Info` button but make those two functions as default and remove the `Match Mode: ` layout, then maybe put the switch here for user to manully control whether seex capture the clpboard.
- [x] then make the `export file` more pro like how it does in productivity, like whic infos/metadatas should be in the csv file, be careful with the encoding.
- [x] last question is for the `Font` tab, should we change it to another name since language is also in this tab.
- [x] make the font size for those input box like `Export directory`, `Library name`, `BOM CSV file ` bigger so that user can see it clearly.
- [x] also align seex with npnp: D:\0000-self-made-rust-tools\npnp, which added a new function for merging the existing libs, but npnp may not do it well, you need consider all of the potential cases and implement it properly.
    - [x] which means seex may need a new tab for such work.
    - [x] the existing libs may not just exported by seex and npnp, it may contain both, or some of the libs are homemade.
- [x] another function may be like seex can read a lib and show some infos that user may be interested in like components id and other metadatas.
- [x] you need implement all of  those improvements above in one or more branches before you merge them to the master, merge is only allowed until i say so.


Implementation: `todo-improvements`. No merge to master has been performed.
Library format support and preservation limits are documented in README.en.md.
