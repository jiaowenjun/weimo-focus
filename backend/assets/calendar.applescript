on run argv
    set calendarName to item 1 of argv
    set eventTitle to item 2 of argv
    set startDate to my makeDate(item 3 of argv, item 4 of argv, item 5 of argv, item 6 of argv, item 7 of argv, item 8 of argv)
    set endDate to my makeDate(item 9 of argv, item 10 of argv, item 11 of argv, item 12 of argv, item 13 of argv, item 14 of argv)
    set eventDescription to item 15 of argv
    set marker to item 16 of argv

    tell application "Calendar"
        set matchingCalendars to calendars whose name is calendarName
        if (count of matchingCalendars) is 0 then
            return "missing"
        end if
        if (count of matchingCalendars) is greater than 1 then
            error "Multiple calendars have the same name: " & calendarName
        end if
        set targetCalendar to item 1 of matchingCalendars

        set matchingEvents to (every event of targetCalendar whose description contains marker)
        if (count of matchingEvents) is greater than 0 then
            return "existing"
        end if

        tell targetCalendar
            make new event at end with properties {summary:eventTitle, start date:startDate, end date:endDate, description:eventDescription}
        end tell
        return "created"
    end tell
end run

on makeDate(yearNumber, monthNumber, dayNumber, hourNumber, minuteNumber, secondNumber)
    set resultDate to current date
    set hours of resultDate to 0
    set minutes of resultDate to 0
    set seconds of resultDate to 0
    set day of resultDate to 1
    set year of resultDate to yearNumber as integer
    set month of resultDate to item (monthNumber as integer) of {January, February, March, April, May, June, July, August, September, October, November, December}
    set day of resultDate to dayNumber as integer
    set hours of resultDate to hourNumber as integer
    set minutes of resultDate to minuteNumber as integer
    set seconds of resultDate to secondNumber as integer
    return resultDate
end makeDate
