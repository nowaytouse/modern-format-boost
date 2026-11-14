on mfbAlbumPathItems(albumPath)
    set savedDelimiters to AppleScript's text item delimiters
    set AppleScript's text item delimiters to "/"
    set pathItems to every text item of albumPath
    set AppleScript's text item delimiters to savedDelimiters
    return pathItems
end mfbAlbumPathItems

on mfbResolveAlbum(pathItems, createMissing)
    if (count of pathItems) is 0 or pathItems contains "" then error "Photos received an empty album path component"
    set parentFolder to missing value
    repeat with componentIndex from 1 to (count of pathItems)
        set componentName to item componentIndex of pathItems
        set wantFolder to componentIndex < (count of pathItems)
        set parentFolder to my mfbResolveAlbumComponent(parentFolder, componentName, wantFolder, createMissing)
        if parentFolder is missing value then return missing value
    end repeat
    return parentFolder
end mfbResolveAlbum

on mfbResolveAlbumComponent(parentFolder, componentName, wantFolder, createMissing)
    tell application "Photos"
        -- An album can hold this level's photos while a same-named folder holds its descendants.
        if parentFolder is missing value then
            if wantFolder then
                set matches to my mfbRootContainers(every folder whose name is componentName)
            else
                set matches to my mfbRootContainers(every album whose name is componentName)
            end if
        else
            -- A Photos folder ID can fail to resolve at application scope.
            if wantFolder then
                set matches to every folder of parentFolder whose name is componentName
            else
                set matches to every album of parentFolder whose name is componentName
            end if
        end if
        set matchedContainer to my mfbUniqueAlbumMatch(matches, componentName)
        if matchedContainer is not missing value then
            considering case
                if (name of matchedContainer) is not componentName then
                    error "Photos destination spelling conflict at “" & componentName & "”. Rename the conflicting folder or album, then resume."
                end if
            end considering
            return matchedContainer
        end if
        if not createMissing then return missing value

        if parentFolder is missing value then
            if wantFolder then
                set createdContainer to make new folder named componentName
            else
                set createdContainer to make new album named componentName
            end if
        else if wantFolder then
            set createdContainer to make new folder at parentFolder named componentName
        else
            set createdContainer to make new album at parentFolder named componentName
        end if
        set createdId to id of createdContainer as text
    end tell
    set resolvedContainer to my mfbResolveAlbumComponent(parentFolder, componentName, wantFolder, false)
    set resolvedId to missing value
    if resolvedContainer is not missing value then
        tell application "Photos" to set resolvedId to id of resolvedContainer as text
    end if
    considering case
        if resolvedId is not createdId then error "Photos destination changed while creating “" & componentName & "”. Resume after checking the destination."
    end considering
    return resolvedContainer
end mfbResolveAlbumComponent

on mfbRootContainers(candidates)
    set rootMatches to {}
    tell application "Photos"
        repeat with candidate in candidates
            if (parent of candidate) is missing value then set end of rootMatches to contents of candidate
        end repeat
    end tell
    return rootMatches
end mfbRootContainers

on mfbUniqueAlbumMatch(matches, componentName)
    if (count of matches) > 1 then
        error "Photos destination name conflict at “" & componentName & "”. Rename the conflicting folder or album, then resume."
    end if
    if (count of matches) is 0 then return missing value
    return item 1 of matches
end mfbUniqueAlbumMatch
